// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use crate::db::Db;
use crate::error::{SResult, not_found};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReviewItem {
    pub record_id: i64,
    pub current_price: i64,
    pub original_price: Option<i64>,
    pub raw_text: Option<String>,
    pub crawled_at: i64,
    pub product_id: i64,
    pub product_name: String,
    pub supermarket_name: String,
    pub url: String,
    pub previous_price: Option<i64>,
}

pub async fn count(db: &Db) -> SResult<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM price_records WHERE needs_review = 1").fetch_one(db).await?)
}

pub async fn list(db: &Db, limit: i64) -> SResult<Vec<ReviewItem>> {
    Ok(sqlx::query_as(
        "SELECT r.id AS record_id, r.current_price, r.original_price, r.raw_text, r.crawled_at, \
                u.product_id, p.name AS product_name, s.name AS supermarket_name, u.url, \
                (SELECT q.current_price FROM price_records q WHERE q.product_url_id = r.product_url_id \
                 AND q.crawled_at < r.crawled_at ORDER BY q.crawled_at DESC LIMIT 1) AS previous_price \
         FROM price_records r JOIN product_urls u ON u.id = r.product_url_id \
         JOIN products p ON p.id = u.product_id JOIN supermarkets s ON s.id = u.supermarket_id \
         WHERE r.needs_review = 1 ORDER BY r.crawled_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(db)
    .await?)
}

pub async fn accept(db: &Db, record_id: i64) -> SResult<()> {
    let done =
        sqlx::query("UPDATE price_records SET needs_review = 0 WHERE id = ?").bind(record_id).execute(db).await?;
    if done.rows_affected() == 0 {
        return not_found(format!("Price #{record_id} not found."));
    }
    tracing::info!(record_id, "price accepted after review");
    Ok(())
}

pub async fn reject(db: &Db, record_id: i64) -> SResult<()> {
    let done = sqlx::query("DELETE FROM price_records WHERE id = ?").bind(record_id).execute(db).await?;
    if done.rows_affected() == 0 {
        return not_found(format!("Price #{record_id} not found."));
    }
    tracing::info!(record_id, "price rejected after review and deleted");
    Ok(())
}
