// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use chrono::{DateTime, Utc};

use crate::cron;
use crate::db::Db;
use crate::error::{SResult, ServiceError, validation};
use crate::services::agent_tasks::{self, QueueResult};
use crate::services::supermarkets::{self, Supermarket};

pub const CRON_OFF_HINT: &str = "Server cron is off. Schedule this in your AI client (e.g. a scheduled task \
that calls trigger_crawl_now then runs Workflow A), or run `mprimcp serve` with MPRIMCP_CRON_ENABLED=true.";

#[derive(Debug, Clone)]
pub struct ScheduleResult {
    pub supermarket: Supermarket,
    pub cron_expression: String,
    pub timezone: String,
    pub next_runs: Vec<DateTime<Utc>>,
}

pub fn preview(expression: &str, timezone: &str, count: usize) -> SResult<Vec<DateTime<Utc>>> {
    cron::parse(expression, timezone)
        .map(|s| s.next_times(Utc::now(), count))
        .map_err(|e| ServiceError::Validation(format!("Invalid cron expression: {e}")))
}

pub async fn set_cron(db: &Db, id_or_name: &str, cron_expression: &str) -> SResult<ScheduleResult> {
    let market = supermarkets::resolve(db, id_or_name).await?;
    let expression = cron::normalize_expression(cron_expression);
    let next_runs = preview(&expression, &market.timezone, 3)?;
    let market = supermarkets::set_schedule_fields(db, &market.id, Some(&expression), true).await?;
    tracing::info!(supermarket = %market.id, %expression, "crawl schedule set");
    Ok(ScheduleResult {
        timezone: market.timezone.clone(),
        supermarket: market,
        cron_expression: expression,
        next_runs,
    })
}

pub async fn disable_cron(db: &Db, id_or_name: &str) -> SResult<Supermarket> {
    let market = supermarkets::resolve(db, id_or_name).await?;
    let market = supermarkets::set_schedule_fields(db, &market.id, market.cron_expression.as_deref(), false).await?;
    tracing::info!(supermarket = %market.id, "crawl schedule paused");
    Ok(market)
}

pub async fn trigger_now(db: &Db, id_or_name: &str) -> SResult<QueueResult> {
    let market = supermarkets::resolve(db, id_or_name).await?;
    let active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM product_urls WHERE supermarket_id = ? AND is_active = 1")
            .bind(&market.id)
            .fetch_one(db)
            .await?;
    if active == 0 {
        return validation(format!("Supermarket '{}' has no active product URLs to crawl yet.", market.id));
    }
    agent_tasks::queue_market(db, market, "manual").await
}

pub fn next_run(market: &Supermarket, server_cron: bool) -> Option<DateTime<Utc>> {
    if !server_cron || !market.cron_enabled {
        return None;
    }
    let expr = market.cron_expression.as_deref()?;
    cron::parse(expr, &market.timezone).ok()?.next_from(Utc::now())
}
