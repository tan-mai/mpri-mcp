// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::auth::oauth;
use crate::cron::{self, Schedule};
use crate::services::{agent_tasks, supermarkets};

const MISFIRE_GRACE: chrono::Duration = chrono::Duration::hours(1);
const MAX_SLEEP: Duration = Duration::from_secs(30);
const HOUSEKEEPING_CRON: &str = "30 4 * * *";
const BACKUP_KEEP: usize = 7;

struct Job {
    expression: String,
    timezone: String,
    schedule: Option<Schedule>,
    next: Option<DateTime<Utc>>,
}

pub async fn housekeeping(app: &App) {
    if let Err(e) = oauth::cleanup_expired(&app.db).await {
        tracing::error!(error = %e, "OAuth cleanup failed");
    }
    if let Err(e) = agent_tasks::maintain(&app.db).await {
        tracing::error!(error = %e, "crawl task maintenance failed");
    }
    match backup(app).await {
        Ok(path) => tracing::info!(path = %path.display(), "SQLite backup written"),
        Err(e) => tracing::error!(error = %e, "SQLite backup failed"),
    }
}

pub async fn backup(app: &App) -> anyhow::Result<std::path::PathBuf> {
    let dir = app.config.data_dir.join("backups");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("mprimcp-{}.db", Utc::now().format("%Y%m%d-%H%M%S")));
    sqlx::query("VACUUM INTO ?").bind(path.to_string_lossy().to_string()).execute(&app.db).await?;
    let mut old: Vec<_> = std::fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("mprimcp-") && n.ends_with(".db"))
        })
        .collect();
    old.sort();
    if old.len() > BACKUP_KEEP {
        for p in &old[..old.len() - BACKUP_KEEP] {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(path)
}

#[derive(Default)]
pub struct CronJobs {
    jobs: HashMap<String, Job>,
}

impl CronJobs {
    async fn sync(&mut self, app: &App, now: DateTime<Utc>) {
        let markets = match supermarkets::list(&app.db).await {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "cannot load schedules");
                return;
            }
        };
        let mut wanted = Vec::new();
        for m in markets {
            let (true, Some(expr)) = (m.cron_enabled, m.cron_expression.clone()) else { continue };
            wanted.push(m.id.clone());
            let changed = self.jobs.get(&m.id).is_none_or(|j| j.expression != expr || j.timezone != m.timezone);
            if changed {
                let schedule = match cron::parse(&expr, &m.timezone) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        tracing::error!(supermarket = %m.id, error = %e, "skipping invalid schedule");
                        None
                    }
                };
                let next = schedule.as_ref().and_then(|s| s.next_from(now));
                tracing::info!(supermarket = %m.id, %expr, ?next, "crawl schedule loaded");
                self.jobs.insert(m.id.clone(), Job { expression: expr, timezone: m.timezone.clone(), schedule, next });
            }
        }
        self.jobs.retain(|id, _| wanted.contains(id));
    }

    pub fn next_due(&self) -> Option<DateTime<Utc>> {
        self.jobs.values().filter_map(|j| j.next).min()
    }

    pub async fn tick(&mut self, app: &App, now: DateTime<Utc>) -> Vec<String> {
        self.sync(app, now).await;
        let mut fired = Vec::new();
        for (id, job) in self.jobs.iter_mut() {
            let (Some(next), Some(schedule)) = (job.next, job.schedule.as_ref()) else { continue };
            if next > now {
                continue;
            }
            if now - next <= MISFIRE_GRACE {
                let result = match supermarkets::get(&app.db, id).await {
                    Ok(Some(m)) => agent_tasks::queue_market(&app.db, m, "cron").await.map(|_| ()),
                    Ok(None) => Ok(()),
                    Err(e) => Err(e),
                };
                match result {
                    Ok(()) => fired.push(id.clone()),
                    Err(e) => tracing::error!(supermarket = %id, error = %e, "scheduled crawl failed"),
                }
            } else {
                tracing::warn!(supermarket = %id, %next, "missed crawl run (more than 1 hour late)");
            }
            job.next = schedule.next_from(now + chrono::Duration::seconds(1));
        }
        fired
    }
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub async fn run(app: App, cancel: CancellationToken) {
    let crawl = app.server_cron();
    let tz = crate::prefs::load(&app.db).await.map(|p| p.tz_name()).unwrap_or(crate::clock::DEFAULT_TIMEZONE);
    let housekeeping_schedule = cron::parse(HOUSEKEEPING_CRON, tz).ok();
    let mut next_housekeeping = housekeeping_schedule.as_ref().and_then(|s| s.next_from(Utc::now()));
    let mut jobs = CronJobs::default();
    tracing::info!(crawl_cron = crawl, "scheduler started");
    loop {
        let now = Utc::now();
        if let Some(at) = next_housekeeping.filter(|at| *at <= now) {
            let app2 = app.clone();
            tokio::spawn(async move { housekeeping(&app2).await });
            next_housekeeping =
                housekeeping_schedule.as_ref().and_then(|s| s.next_from(at.max(now) + chrono::Duration::seconds(1)));
        }
        if crawl {
            jobs.tick(&app, now).await;
        }
        let earliest = jobs.next_due().into_iter().chain(next_housekeeping).min();
        let wait =
            earliest.map(|t| (t - Utc::now()).to_std().unwrap_or(Duration::ZERO)).unwrap_or(MAX_SLEEP).min(MAX_SLEEP);
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(wait.max(Duration::from_millis(200))) => {}
        }
    }
    tracing::info!("scheduler stopped");
}
