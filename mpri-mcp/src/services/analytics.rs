// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::{BTreeMap, HashMap};

use chrono::NaiveDate;
use sqlx::{FromRow, SqliteConnection};

use crate::clock::{DAY_MS, local_date, now_ms};
use crate::db::{Db, json_ids};
use crate::error::{SResult, validation};
use crate::services::products::{self, Product, ProductUrl};
use crate::services::supermarkets;
use crate::text::{FuzzyMatch, round1};

pub const STALE_AFTER_MS: i64 = 3 * DAY_MS;

#[derive(Debug, Clone, FromRow)]
pub struct PriceRecord {
    pub id: i64,
    pub product_url_id: i64,
    pub current_price: i64,
    pub original_price: Option<i64>,
    pub member_price: Option<i64>,
    pub in_stock: Option<bool>,
    pub source: String,
    pub needs_review: bool,
    pub raw_text: Option<String>,
    pub crawled_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct MarketPrice {
    pub supermarket_id: String,
    pub supermarket_name: String,
    pub url: String,
    pub url_active: bool,
    pub current_price: Option<i64>,
    pub original_price: Option<i64>,
    pub member_price: Option<i64>,
    pub discount_pct: Option<f64>,
    pub member_discount_pct: Option<f64>,
    pub in_stock: Option<bool>,
    pub crawled_at: Option<i64>,
    pub is_stale: bool,
    pub last_error: Option<String>,
    pub source: Option<String>,
    pub needs_review: bool,
}

#[derive(Debug, Clone)]
pub struct Comparison {
    pub product: Product,
    pub prices: Vec<MarketPrice>,
    pub cheapest: Option<MarketPrice>,
    pub alternatives: Vec<FuzzyMatch<i64>>,
}

#[derive(Debug, Clone)]
pub struct DiscountItem {
    pub product_id: i64,
    pub product_name: String,
    pub category: Option<String>,
    pub supermarket_id: String,
    pub supermarket_name: String,
    pub url: String,
    pub current_price: i64,
    pub original_price: Option<i64>,
    pub member_price: Option<i64>,
    pub discount_pct: f64,
    pub member_discount_pct: Option<f64>,
    pub crawled_at: i64,
}

#[derive(FromRow)]
struct DiscountRow {
    url_id: i64,
    product_id: i64,
    product_name: String,
    category: Option<String>,
    supermarket_id: String,
    supermarket_name: String,
    url: String,
}

#[derive(Debug, Clone)]
pub struct HistoryPoint {
    pub day: NaiveDate,
    pub current_price: i64,
    pub original_price: Option<i64>,
    pub member_price: Option<i64>,
    pub needs_review: bool,
}

#[derive(Debug, Clone)]
pub struct MarketHistory {
    pub supermarket_id: String,
    pub supermarket_name: String,
    pub points: Vec<HistoryPoint>,
}

impl MarketHistory {
    pub fn min_point(&self) -> Option<&HistoryPoint> {
        self.points.iter().min_by_key(|p| (p.current_price, p.day))
    }

    pub fn max_price(&self) -> Option<i64> {
        self.points.iter().map(|p| p.current_price).max()
    }

    pub fn avg_price(&self) -> Option<i64> {
        if self.points.is_empty() {
            return None;
        }
        let sum: i64 = self.points.iter().map(|p| p.current_price).sum();
        Some((sum as f64 / self.points.len() as f64).round_ties_even() as i64)
    }

    pub fn latest(&self) -> Option<&HistoryPoint> {
        self.points.last()
    }
}

#[derive(Debug, Clone)]
pub struct History {
    pub product: Product,
    pub days: i64,
    pub series: Vec<MarketHistory>,
    pub alternatives: Vec<FuzzyMatch<i64>>,
}

pub fn discount_pct(original: Option<i64>, price: Option<i64>) -> Option<f64> {
    let (price, original) = (price?, original?);
    if original <= 0 {
        return None;
    }
    if original <= price {
        return Some(0.0);
    }
    Some(round1((original - price) as f64 / original as f64 * 100.0))
}

pub async fn latest_records(
    conn: &mut SqliteConnection,
    url_ids: Option<&[i64]>,
    since: Option<i64>,
) -> SResult<HashMap<i64, PriceRecord>> {
    let sql = "SELECT p.* FROM price_records p JOIN (SELECT product_url_id, MAX(crawled_at) AS mx          FROM price_records WHERE (?1 IS NULL OR product_url_id IN (SELECT value FROM json_each(?1)))          AND (?2 IS NULL OR crawled_at >= ?2) GROUP BY product_url_id) m          ON p.product_url_id = m.product_url_id AND p.crawled_at = m.mx ORDER BY p.id";
    let rows: Vec<PriceRecord> = sqlx::query_as(sql).bind(url_ids.map(json_ids)).bind(since).fetch_all(conn).await?;
    Ok(rows.into_iter().map(|r| (r.product_url_id, r)).collect())
}

fn market_price(u: &ProductUrl, rec: Option<&PriceRecord>, now: i64) -> MarketPrice {
    let mut mp = MarketPrice {
        supermarket_id: u.supermarket_id.clone(),
        supermarket_name: u.supermarket_name.clone(),
        url: u.url.clone(),
        url_active: u.is_active,
        last_error: if u.consecutive_failures > 0 { u.last_error.clone() } else { None },
        ..Default::default()
    };
    if let Some(rec) = rec {
        mp.current_price = Some(rec.current_price);
        mp.original_price = rec.original_price;
        mp.member_price = rec.member_price;
        mp.discount_pct = discount_pct(rec.original_price, Some(rec.current_price));
        mp.member_discount_pct = rec
            .member_price
            .filter(|m| *m != 0)
            .and_then(|m| discount_pct(Some(rec.original_price.unwrap_or(rec.current_price)), Some(m)));
        mp.in_stock = rec.in_stock;
        mp.crawled_at = Some(rec.crawled_at);
        mp.is_stale = now - rec.crawled_at > STALE_AFTER_MS;
        mp.source = Some(rec.source.clone());
        mp.needs_review = rec.needs_review;
    }
    if u.in_stock == Some(false) {
        mp.in_stock = Some(false);
    }
    mp
}

pub async fn latest_prices(db: &Db, product_id: i64) -> SResult<Vec<MarketPrice>> {
    let urls = products::urls_of(db, product_id).await?;
    let ids: Vec<i64> = urls.iter().map(|u| u.id).collect();
    let mut conn = db.acquire().await?;
    let latest = latest_records(&mut conn, Some(&ids), None).await?;
    let now = now_ms();
    let mut prices: Vec<MarketPrice> = urls.iter().map(|u| market_price(u, latest.get(&u.id), now)).collect();
    prices.sort_by(|a, b| {
        (a.current_price.is_none(), a.current_price.unwrap_or(0), &a.supermarket_name).cmp(&(
            b.current_price.is_none(),
            b.current_price.unwrap_or(0),
            &b.supermarket_name,
        ))
    });
    Ok(prices)
}

pub async fn cheapest_by_product(db: &Db) -> SResult<HashMap<i64, MarketPrice>> {
    let mut conn = db.acquire().await?;
    let latest = latest_records(&mut conn, None, None).await?;
    if latest.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<i64> = latest.keys().copied().collect();
    let urls: Vec<ProductUrl> = sqlx::query_as(concat!(
        crate::select_urls!(),
        " WHERE u.id IN (SELECT value FROM json_each(?)) AND u.is_active = 1 ORDER BY u.id"
    ))
    .bind(json_ids(&ids))
    .fetch_all(&mut *conn)
    .await?;
    let now = now_ms();
    let mut cheapest: HashMap<i64, MarketPrice> = HashMap::new();
    for u in &urls {
        let mp = market_price(u, latest.get(&u.id), now);
        let Some(price) = mp.current_price else { continue };
        let better = cheapest.get(&u.product_id).and_then(|b| b.current_price).is_none_or(|best| price < best);
        if better {
            cheapest.insert(u.product_id, mp);
        }
    }
    Ok(cheapest)
}

pub async fn price_comparison(db: &Db, product_name: &str) -> SResult<Comparison> {
    let (product, alternatives) = products::resolve_product(db, product_name).await?;
    let prices = latest_prices(db, product.id).await?;
    let cheapest = prices.iter().find(|p| p.current_price.is_some()).cloned();
    Ok(Comparison { product, prices, cheapest, alternatives })
}

pub async fn deep_discounts(db: &Db, min_pct: f64, days: i64, limit: i64) -> SResult<Vec<DiscountItem>> {
    if !(min_pct > 0.0 && min_pct < 100.0) {
        return validation("min_discount_percentage must be between 1 and 99.");
    }
    if !(1..=365).contains(&days) {
        return validation("days must be between 1 and 365.");
    }
    let since = now_ms() - days * DAY_MS;
    let mut conn = db.acquire().await?;
    let latest = latest_records(&mut conn, None, Some(since)).await?;
    if latest.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = latest.keys().copied().collect();
    let rows: Vec<DiscountRow> = sqlx::query_as(
        "SELECT u.id AS url_id, u.product_id, p.name AS product_name, p.category, u.supermarket_id,          s.name AS supermarket_name, u.url \
         FROM product_urls u JOIN products p ON p.id = u.product_id \
         JOIN supermarkets s ON s.id = u.supermarket_id \
         WHERE u.id IN (SELECT value FROM json_each(?)) AND u.is_active = 1",
    )
    .bind(json_ids(&ids))
    .fetch_all(&mut *conn)
    .await?;
    let mut items = Vec::new();
    for DiscountRow { url_id, product_id, product_name, category, supermarket_id, supermarket_name, url } in rows {
        let rec = &latest[&url_id];
        if rec.needs_review {
            continue;
        }
        let Some(pct) = discount_pct(rec.original_price, Some(rec.current_price)) else { continue };
        if pct < min_pct {
            continue;
        }
        items.push(DiscountItem {
            product_id,
            product_name,
            category,
            supermarket_id,
            supermarket_name,
            url,
            current_price: rec.current_price,
            original_price: rec.original_price,
            member_price: rec.member_price,
            discount_pct: pct,
            member_discount_pct: rec
                .member_price
                .filter(|m| *m != 0)
                .and_then(|m| discount_pct(rec.original_price, Some(m))),
            crawled_at: rec.crawled_at,
        });
    }
    items.sort_by(|a, b| {
        b.discount_pct
            .partial_cmp(&a.discount_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.product_name.cmp(&b.product_name))
    });
    items.truncate(limit.clamp(1, 500) as usize);
    Ok(items)
}

pub async fn product_history(db: &Db, product: Product, days: i64, supermarket_id: Option<&str>) -> SResult<History> {
    if !(1..=730).contains(&days) {
        return validation("days must be between 1 and 730.");
    }
    let wanted = match supermarket_id {
        None => None,
        Some(raw) => Some(match supermarkets::find(db, raw).await? {
            Some(m) => m.id,
            None => raw.to_string(),
        }),
    };
    let since = now_ms() - days * DAY_MS;
    let mut urls: Vec<ProductUrl> = products::urls_of(db, product.id)
        .await?
        .into_iter()
        .filter(|u| wanted.as_deref().is_none_or(|w| w == u.supermarket_id))
        .collect();
    urls.sort_by(|a, b| a.supermarket_name.cmp(&b.supermarket_name));
    let ids: Vec<i64> = urls.iter().map(|u| u.id).collect();
    let records: Vec<PriceRecord> = sqlx::query_as(
        "SELECT * FROM price_records WHERE product_url_id IN (SELECT value FROM json_each(?)) \
         AND crawled_at >= ? ORDER BY crawled_at, id",
    )
    .bind(json_ids(&ids))
    .bind(since)
    .fetch_all(db)
    .await?;
    let mut by_url: HashMap<i64, BTreeMap<NaiveDate, PriceRecord>> = HashMap::new();
    for rec in records {
        by_url.entry(rec.product_url_id).or_default().insert(local_date(rec.crawled_at), rec);
    }
    let series = urls
        .iter()
        .map(|u| MarketHistory {
            supermarket_id: u.supermarket_id.clone(),
            supermarket_name: u.supermarket_name.clone(),
            points: by_url
                .get(&u.id)
                .map(|days| {
                    days.iter()
                        .map(|(d, r)| HistoryPoint {
                            day: *d,
                            current_price: r.current_price,
                            original_price: r.original_price,
                            member_price: r.member_price,
                            needs_review: r.needs_review,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect();
    Ok(History { product, days, series, alternatives: Vec::new() })
}

pub async fn price_history(db: &Db, product_name: &str, days: i64, supermarket_id: Option<&str>) -> SResult<History> {
    let (product, alternatives) = products::resolve_product(db, product_name).await?;
    let mut history = product_history(db, product, days, supermarket_id).await?;
    history.alternatives = alternatives;
    Ok(history)
}

pub async fn has_any_price(db: &Db) -> SResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM price_records)").fetch_one(db).await?)
}
