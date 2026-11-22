// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::{BTreeSet, HashMap, HashSet};

use rand::RngCore;
use sqlx::{FromRow, SqliteConnection};

use crate::clock::{HOUR_MS, MINUTE_MS, now_ms};
use crate::db::{self, Db, json_ids};
use crate::error::{SResult, validation};
use crate::services::alerts::SavedPrice;
use crate::services::analytics::latest_records;
use crate::services::supermarkets::{self, Supermarket};
use crate::text::round1;

pub const LEASE_MS: i64 = 30 * MINUTE_MS;
pub const EXPIRE_HOURS: i64 = 48;
pub const MAX_ATTEMPTS: i64 = 3;
pub const MAX_LIMIT: i64 = 50;
pub const MAX_BATCH: usize = 50;
pub const MAX_PLAUSIBLE_PRICE: i64 = 1_000_000_000;
pub const REVIEW_CHANGE: f64 = 0.5;
pub const RAW_TEXT_MAX: usize = 500;
pub const NOTE_MAX: usize = 300;
pub const RESULT_STATUSES: [&str; 4] = ["ok", "out_of_stock", "not_found", "error"];

#[derive(Debug, Clone, FromRow)]
pub struct CrawlTask {
    pub id: String,
    pub product_url_id: i64,
    pub supermarket_id: String,
    pub run_id: Option<i64>,
    pub url: String,
    pub status: String,
    pub attempts: i64,
    pub created_at: i64,
    pub claimed_at: Option<i64>,
    pub lease_expires_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QueueResult {
    pub supermarket: Supermarket,
    pub run_id: Option<i64>,
    pub queued: i64,
    pub already_open: i64,
    pub open_tasks: i64,
}

#[derive(Debug, Clone)]
pub struct ClaimedTask {
    pub task: CrawlTask,
    pub product_name: String,
    pub category: Option<String>,
    pub supermarket_name: String,
    pub last_price: Option<i64>,
    pub last_original_price: Option<i64>,
    pub last_checked_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ClaimResult {
    pub tasks: Vec<ClaimedTask>,
    pub remaining_pending: i64,
}

#[derive(Debug, Clone, Default)]
pub struct PriceResult {
    pub task_id: String,
    pub status: String,
    pub price: Option<i64>,
    pub original_price: Option<i64>,
    pub member_price: Option<i64>,
    pub raw_text: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Flagged {
    pub task_id: String,
    pub product_name: String,
    pub supermarket_id: String,
    pub price: i64,
    pub last_price: i64,
    pub change_pct: f64,
}

#[derive(Debug, Clone)]
pub struct Rejected {
    pub task_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct SubmitResult {
    pub accepted: i64,
    pub requeued: i64,
    pub flagged: Vec<Flagged>,
    pub rejected: Vec<Rejected>,
    pub warnings: Vec<String>,
    pub finished_runs: Vec<i64>,
    pub remaining_pending: i64,
    pub saved: Vec<SavedPrice>,
    pub queue_empty: bool,
}

fn new_task_id() -> String {
    let mut bytes = [0u8; 6];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

async fn count_pending(conn: &mut SqliteConnection, supermarket_id: Option<&str>) -> SResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM crawl_tasks WHERE status = 'pending' AND (?1 IS NULL OR supermarket_id = ?1)",
    )
    .bind(supermarket_id)
    .fetch_one(conn)
    .await?)
}

async fn maintain_in(conn: &mut SqliteConnection, now: i64) -> SResult<()> {
    let released = sqlx::query(
        "UPDATE crawl_tasks SET status = 'pending', lease_expires_at = NULL \
         WHERE status = 'claimed' AND lease_expires_at < ?",
    )
    .bind(now)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if released > 0 {
        tracing::info!(released, "released crawl tasks whose lease expired");
    }
    let cutoff = now - EXPIRE_HOURS * HOUR_MS;
    let run_ids: Vec<Option<i64>> =
        sqlx::query_scalar("SELECT DISTINCT run_id FROM crawl_tasks WHERE status = 'pending' AND created_at < ?")
            .bind(cutoff)
            .fetch_all(&mut *conn)
            .await?;
    let expired = sqlx::query(
        "UPDATE crawl_tasks SET status = 'expired', completed_at = ?, last_error = ? \
         WHERE status = 'pending' AND created_at < ?",
    )
    .bind(now)
    .bind(format!("Not crawled within {EXPIRE_HOURS} hours"))
    .bind(cutoff)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if expired > 0 {
        tracing::warn!(expired, "expired crawl tasks never crawled");
        for run_id in run_ids.into_iter().flatten() {
            finish_run_if_done(conn, run_id, now).await?;
        }
    }
    Ok(())
}

async fn finish_run_if_done(conn: &mut SqliteConnection, run_id: i64, now: i64) -> SResult<bool> {
    let counts: HashMap<String, i64> =
        sqlx::query_as::<_, (String, i64)>("SELECT status, COUNT(*) FROM crawl_tasks WHERE run_id = ? GROUP BY status")
            .bind(run_id)
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect();
    let get = |s: &str| counts.get(s).copied().unwrap_or(0);
    if get("pending") > 0 || get("claimed") > 0 {
        return Ok(false);
    }
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM crawl_runs WHERE id = ?")
        .bind(run_id)
        .fetch_optional(&mut *conn)
        .await?;
    if status.as_deref() != Some("queued") {
        return Ok(false);
    }
    let ok = get("done");
    let failed = get("failed") + get("expired");
    let outcome = if failed == 0 {
        "success"
    } else if ok > 0 {
        "partial"
    } else {
        "failed"
    };
    sqlx::query(
        "UPDATE crawl_runs SET urls_ok = ?, urls_failed = ?, status = ?, trigger = 'agent', \
         finished_at = ? WHERE id = ?",
    )
    .bind(ok)
    .bind(failed)
    .bind(outcome)
    .bind(now)
    .bind(run_id)
    .execute(&mut *conn)
    .await?;
    tracing::info!(run_id, outcome, ok, failed, "agent crawl run finished");
    Ok(true)
}

pub async fn maintain(db: &Db) -> SResult<()> {
    let mut tx = db::begin(db).await?;
    maintain_in(&mut tx, now_ms()).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn queue_market(db: &Db, market: Supermarket, trigger: &str) -> SResult<QueueResult> {
    let now = now_ms();
    let mut tx = db::begin(db).await?;
    maintain_in(&mut tx, now).await?;
    let urls: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, url FROM product_urls WHERE supermarket_id = ? AND is_active = 1 ORDER BY id")
            .bind(&market.id)
            .fetch_all(&mut *tx)
            .await?;
    let open: HashSet<i64> = sqlx::query_scalar(
        "SELECT product_url_id FROM crawl_tasks WHERE supermarket_id = ? AND status IN ('pending', 'claimed')",
    )
    .bind(&market.id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .collect();
    let new: Vec<&(i64, String)> = urls.iter().filter(|(id, _)| !open.contains(id)).collect();
    let mut run_id = None;
    if !new.is_empty() {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO crawl_runs (supermarket_id, trigger, status, started_at, urls_total) \
             VALUES (?, ?, 'queued', ?, ?) RETURNING id",
        )
        .bind(&market.id)
        .bind(trigger)
        .bind(now)
        .bind(new.len() as i64)
        .fetch_one(&mut *tx)
        .await?;
        run_id = Some(id);
        for (url_id, url) in &new {
            sqlx::query(
                "INSERT INTO crawl_tasks (id, product_url_id, supermarket_id, run_id, url, status, attempts, \
                 created_at) VALUES (?, ?, ?, ?, ?, 'pending', 0, ?)",
            )
            .bind(new_task_id())
            .bind(url_id)
            .bind(&market.id)
            .bind(id)
            .bind(url)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    let queued = new.len() as i64;
    let already_open = urls.len() as i64 - queued;
    tracing::info!(supermarket = %market.id, trigger, queued, already_open, ?run_id, "queued crawl tasks");
    Ok(QueueResult { open_tasks: open.len() as i64 + queued, supermarket: market, run_id, queued, already_open })
}

pub async fn claim_tasks(db: &Db, supermarket: Option<&str>, limit: i64) -> SResult<ClaimResult> {
    if !(1..=MAX_LIMIT).contains(&limit) {
        return validation(format!("limit must be between 1 and {MAX_LIMIT}."));
    }
    let market_id = match supermarket.filter(|s| !s.is_empty()) {
        Some(s) => Some(supermarkets::resolve(db, s).await?.id),
        None => None,
    };
    let now = now_ms();
    let lease_until = now + LEASE_MS;
    let mut tx = db::begin(db).await?;
    maintain_in(&mut tx, now).await?;
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM crawl_tasks WHERE status = 'pending' AND (?1 IS NULL OR supermarket_id = ?1) \
         ORDER BY created_at, product_url_id LIMIT ?2",
    )
    .bind(&market_id)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    let mut claimed = Vec::new();
    if !ids.is_empty() {
        sqlx::query(
            "UPDATE crawl_tasks SET status = 'claimed', claimed_at = ?, lease_expires_at = ?, \
             attempts = attempts + 1 WHERE id IN (SELECT value FROM json_each(?)) AND status = 'pending'",
        )
        .bind(now)
        .bind(lease_until)
        .bind(json_ids(&ids))
        .execute(&mut *tx)
        .await?;
        let rows: Vec<(String, String, Option<String>, String)> = sqlx::query_as(
            "SELECT t.id, p.name, p.category, s.name FROM crawl_tasks t \
             JOIN product_urls u ON u.id = t.product_url_id JOIN products p ON p.id = u.product_id \
             JOIN supermarkets s ON s.id = t.supermarket_id \
             WHERE t.id IN (SELECT value FROM json_each(?)) AND t.status = 'claimed'",
        )
        .bind(json_ids(&ids))
        .fetch_all(&mut *tx)
        .await?;
        let info: HashMap<String, (String, Option<String>, String)> =
            rows.into_iter().map(|(id, p, c, s)| (id, (p, c, s))).collect();
        let tasks: Vec<CrawlTask> =
            sqlx::query_as("SELECT * FROM crawl_tasks WHERE id IN (SELECT value FROM json_each(?))")
                .bind(json_ids(&ids))
                .fetch_all(&mut *tx)
                .await?;
        let mut by_id: HashMap<String, CrawlTask> = tasks.into_iter().map(|t| (t.id.clone(), t)).collect();
        let url_ids: Vec<i64> = by_id.values().map(|t| t.product_url_id).collect();
        let latest = latest_records(&mut tx, Some(&url_ids), None).await?;
        for id in &ids {
            let (Some(task), Some((product_name, category, supermarket_name))) = (by_id.remove(id), info.get(id))
            else {
                continue;
            };
            let rec = latest.get(&task.product_url_id);
            claimed.push(ClaimedTask {
                product_name: product_name.clone(),
                category: category.clone(),
                supermarket_name: supermarket_name.clone(),
                last_price: rec.map(|r| r.current_price),
                last_original_price: rec.and_then(|r| r.original_price),
                last_checked_at: rec.map(|r| r.crawled_at),
                task,
            });
        }
    }
    let remaining = count_pending(&mut tx, market_id.as_deref()).await?;
    tx.commit().await?;
    tracing::info!(claimed = claimed.len(), supermarket = ?market_id, remaining, "claimed crawl tasks");
    Ok(ClaimResult { tasks: claimed, remaining_pending: remaining })
}

struct TaskRow {
    task: CrawlTask,
    product_id: i64,
    product_name: String,
    supermarket_name: String,
}

fn problem(r: &PriceResult, task: Option<&CrawlTask>, seen: &HashSet<String>) -> Option<String> {
    if seen.contains(&r.task_id) {
        return Some("Duplicate task_id in this batch.".into());
    }
    let Some(task) = task else { return Some("Unknown task_id.".into()) };
    if task.status != "claimed" && !(task.status == "pending" && task.claimed_at.is_some()) {
        if task.status == "pending" {
            return Some("Task is pending; claim it with get_pending_crawl_tasks first.".into());
        }
        return Some(format!("Task is already {}.", task.status));
    }
    if !RESULT_STATUSES.contains(&r.status.as_str()) {
        return Some(format!("status must be one of: {}.", RESULT_STATUSES.join(", ")));
    }
    if r.raw_text.as_ref().is_some_and(|t| t.chars().count() > RAW_TEXT_MAX) {
        return Some(format!("raw_text is longer than {RAW_TEXT_MAX} characters."));
    }
    if r.note.as_ref().is_some_and(|t| t.chars().count() > NOTE_MAX) {
        return Some(format!("note is longer than {NOTE_MAX} characters."));
    }
    if r.status != "ok" {
        return None;
    }
    let price = match r.price {
        Some(p) if p > 0 => p,
        _ => return Some("status 'ok' needs a price greater than 0.".into()),
    };
    if price > MAX_PLAUSIBLE_PRICE {
        return Some(format!("price {price} is implausibly high."));
    }
    if let Some(original) = r.original_price.filter(|o| *o < price) {
        return Some(format!("original_price ({original}) is lower than price ({price})."));
    }
    if r.member_price.is_some_and(|m| m <= 0) {
        return Some("member_price must be greater than 0.".into());
    }
    None
}

async fn mark_success(conn: &mut SqliteConnection, url_id: i64, now: i64, in_stock: bool) -> SResult<()> {
    sqlx::query(
        "UPDATE product_urls SET last_crawled_at = ?1, last_success_at = ?1, last_error = NULL, \
         consecutive_failures = 0, in_stock = ?2 WHERE id = ?3",
    )
    .bind(now)
    .bind(in_stock)
    .bind(url_id)
    .execute(conn)
    .await?;
    Ok(())
}

async fn mark_failure(conn: &mut SqliteConnection, url_id: i64, now: i64, reason: &str) -> SResult<()> {
    sqlx::query(
        "UPDATE product_urls SET last_crawled_at = ?, last_error = ?, \
         consecutive_failures = consecutive_failures + 1 WHERE id = ?",
    )
    .bind(now)
    .bind(reason)
    .bind(url_id)
    .execute(conn)
    .await?;
    Ok(())
}

async fn end_task(
    conn: &mut SqliteConnection,
    task_id: &str,
    status: &str,
    now: i64,
    note: Option<&str>,
) -> SResult<()> {
    sqlx::query(
        "UPDATE crawl_tasks SET status = ?, completed_at = ?, lease_expires_at = NULL, last_error = ? WHERE id = ?",
    )
    .bind(status)
    .bind(now)
    .bind(if status == "failed" { note } else { None })
    .bind(task_id)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn submit_results(db: &Db, results: &[PriceResult]) -> SResult<SubmitResult> {
    if results.is_empty() {
        return validation("results is empty.");
    }
    if results.len() > MAX_BATCH {
        return validation(format!("Send at most {MAX_BATCH} results per call."));
    }
    let now = now_ms();
    let mut out = SubmitResult::default();
    let mut runs: BTreeSet<i64> = BTreeSet::new();
    let mut tx = db::begin(db).await?;
    maintain_in(&mut tx, now).await?;
    let ids: Vec<&str> = results.iter().map(|r| r.task_id.as_str()).collect();
    let tasks: Vec<CrawlTask> =
        sqlx::query_as("SELECT * FROM crawl_tasks WHERE id IN (SELECT value FROM json_each(?))")
            .bind(json_ids(&ids))
            .fetch_all(&mut *tx)
            .await?;
    let info: HashMap<String, (i64, String, String)> = sqlx::query_as::<_, (String, i64, String, String)>(
        "SELECT t.id, p.id, p.name, s.name FROM crawl_tasks t \
         JOIN product_urls u ON u.id = t.product_url_id JOIN products p ON p.id = u.product_id \
         JOIN supermarkets s ON s.id = t.supermarket_id WHERE t.id IN (SELECT value FROM json_each(?))",
    )
    .bind(json_ids(&ids))
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|(t, pid, pname, sname)| (t, (pid, pname, sname)))
    .collect();
    let mut rows: HashMap<String, TaskRow> = HashMap::new();
    for task in tasks {
        if let Some((product_id, product_name, supermarket_name)) = info.get(&task.id).cloned() {
            rows.insert(task.id.clone(), TaskRow { task, product_id, product_name, supermarket_name });
        }
    }
    let url_ids: Vec<i64> = rows.values().map(|r| r.task.product_url_id).collect();
    let latest = latest_records(&mut tx, Some(&url_ids), None).await?;
    let mut seen: HashSet<String> = HashSet::new();
    for r in results {
        let reason = problem(r, rows.get(&r.task_id).map(|row| &row.task), &seen);
        seen.insert(r.task_id.clone());
        let Some(row) = rows.get_mut(&r.task_id).filter(|_| reason.is_none()) else {
            let reason = reason.unwrap_or_else(|| "Unknown task_id.".into());
            tracing::warn!(task = %r.task_id, %reason, "rejected agent result");
            out.rejected.push(Rejected { task_id: r.task_id.clone(), reason });
            continue;
        };
        let task = &mut row.task;
        let url_id = task.product_url_id;
        let note = r.note.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
        out.accepted += 1;
        match r.status.as_str() {
            "ok" => {
                let price = r.price.unwrap_or_default();
                let last = latest.get(&url_id);
                let change = last
                    .filter(|l| l.current_price > 0)
                    .map(|l| (price - l.current_price) as f64 / l.current_price as f64);
                let flagged = change.is_some_and(|c| c.abs() > REVIEW_CHANGE);
                let mut member = r.member_price;
                if let Some(m) = member.filter(|m| *m >= price) {
                    out.warnings.push(format!("{}: member_price {m} is not below price {price}; ignored.", task.id));
                    member = None;
                }
                let original = r.original_price.filter(|o| *o != 0).unwrap_or(price);
                let raw_text = r.raw_text.as_deref().map(str::trim).filter(|t| !t.is_empty());
                sqlx::query(
                    "INSERT INTO price_records (product_url_id, current_price, original_price, member_price, \
                     in_stock, source, needs_review, raw_text, crawled_at) VALUES (?, ?, ?, ?, 1, 'agent', ?, ?, ?)",
                )
                .bind(url_id)
                .bind(price)
                .bind(original)
                .bind(member)
                .bind(flagged)
                .bind(raw_text)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                mark_success(&mut tx, url_id, now, true).await?;
                end_task(&mut tx, &task.id, "done", now, note.as_deref()).await?;
                task.status = "done".into();
                let product_url: (String, String) =
                    sqlx::query_as("SELECT supermarket_id, url FROM product_urls WHERE id = ?")
                        .bind(url_id)
                        .fetch_one(&mut *tx)
                        .await?;
                out.saved.push(SavedPrice {
                    product_id: row.product_id,
                    product_name: row.product_name.clone(),
                    supermarket_id: product_url.0,
                    supermarket_name: row.supermarket_name.clone(),
                    url: product_url.1,
                    price,
                    original_price: Some(original),
                    member_price: member,
                    needs_review: flagged,
                });
                if let (true, Some(last), Some(change)) = (flagged, last, change) {
                    out.flagged.push(Flagged {
                        task_id: task.id.clone(),
                        product_name: row.product_name.clone(),
                        supermarket_id: task.supermarket_id.clone(),
                        price,
                        last_price: last.current_price,
                        change_pct: round1(change * 100.0),
                    });
                    tracing::warn!(task = %task.id, product = %row.product_name, price,
                        last = last.current_price, "flagged price for review");
                }
            }
            "out_of_stock" => {
                mark_success(&mut tx, url_id, now, false).await?;
                let stock_note = match &note {
                    Some(n) => format!("Out of stock: {n}"),
                    None => "Out of stock".into(),
                };
                end_task(&mut tx, &task.id, "done", now, Some(&stock_note)).await?;
                task.status = "done".into();
            }
            "not_found" => {
                let reason = match &note {
                    Some(n) => format!("Page not found: {n}"),
                    None => "Page not found or product removed".into(),
                };
                mark_failure(&mut tx, url_id, now, &reason).await?;
                end_task(&mut tx, &task.id, "failed", now, Some(&reason)).await?;
                task.status = "failed".into();
            }
            _ => {
                let reason = note.clone().unwrap_or_else(|| "The agent could not read the price".into());
                if task.attempts < MAX_ATTEMPTS {
                    sqlx::query(
                        "UPDATE crawl_tasks SET status = 'pending', lease_expires_at = NULL, last_error = ? WHERE id = ?",
                    )
                    .bind(&reason)
                    .bind(&task.id)
                    .execute(&mut *tx)
                    .await?;
                    task.status = "pending".into();
                    out.requeued += 1;
                } else {
                    mark_failure(&mut tx, url_id, now, &reason).await?;
                    end_task(&mut tx, &task.id, "failed", now, Some(&reason)).await?;
                    task.status = "failed".into();
                }
            }
        }
        if let Some(run_id) = task.run_id.filter(|_| matches!(task.status.as_str(), "done" | "failed" | "expired")) {
            runs.insert(run_id);
        }
    }
    for run_id in runs {
        if finish_run_if_done(&mut tx, run_id, now).await? {
            out.finished_runs.push(run_id);
        }
    }
    out.remaining_pending = count_pending(&mut tx, None).await?;
    let open: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crawl_tasks WHERE status IN ('pending', 'claimed')")
        .fetch_one(&mut *tx)
        .await?;
    out.queue_empty = open == 0;
    tx.commit().await?;
    tracing::info!(
        accepted = out.accepted,
        flagged = out.flagged.len(),
        requeued = out.requeued,
        rejected = out.rejected.len(),
        pending = out.remaining_pending,
        "agent results submitted"
    );
    Ok(out)
}

pub async fn pending_total(db: &Db) -> SResult<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM crawl_tasks WHERE status IN ('pending', 'claimed')")
        .fetch_one(db)
        .await?)
}
