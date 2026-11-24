// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::{BTreeMap, HashMap};

use sqlx::SqliteConnection;

use crate::clock::now_ms;
use crate::db::{self, Db, json_ids};
use crate::error::{SResult, ServiceError, not_found, validation};
use crate::services::analytics::{MarketPrice, latest_prices};
use crate::services::products::{self, Product};
use crate::services::supermarkets;
use crate::text::fuzzy_rank;

pub const MAX_TARGET: i64 = 1_000_000_000;

#[derive(Debug, Clone)]
pub struct SavedPrice {
    pub product_id: i64,
    pub product_name: String,
    pub supermarket_id: String,
    pub supermarket_name: String,
    pub url: String,
    pub price: i64,
    pub original_price: Option<i64>,
    pub member_price: Option<i64>,
    pub needs_review: bool,
}

#[derive(Debug, Clone)]
pub struct AlertHit {
    pub product_id: i64,
    pub product_name: String,
    pub target_price: i64,
    pub prices: Vec<SavedPrice>,
}

#[derive(Debug, Clone)]
pub struct Alert {
    pub id: i64,
    pub product_id: i64,
    pub target_price: i64,
    pub supermarket_ids: Option<Vec<String>>,
}

impl Alert {
    pub fn covers(&self, supermarket_id: &str) -> bool {
        self.supermarket_ids.as_ref().is_none_or(|ids| ids.iter().any(|i| i == supermarket_id))
    }
}

#[derive(Debug, Clone)]
pub struct AlertView {
    pub alert: Alert,
    pub product: Product,
    pub best: Option<MarketPrice>,
}

impl AlertView {
    pub fn below(&self) -> bool {
        self.best.as_ref().and_then(|b| b.current_price).is_some_and(|p| p < self.alert.target_price)
    }
}

async fn load_alerts(conn: &mut SqliteConnection, product_ids: Option<&[i64]>) -> SResult<Vec<Alert>> {
    let rows: Vec<(i64, i64, i64, bool)> = sqlx::query_as(
        "SELECT id, product_id, target_price, all_supermarkets FROM price_alerts \
         WHERE ?1 IS NULL OR product_id IN (SELECT value FROM json_each(?1)) ORDER BY id",
    )
    .bind(product_ids.map(json_ids))
    .fetch_all(&mut *conn)
    .await?;
    let scopes: Vec<(i64, String)> =
        sqlx::query_as("SELECT alert_id, supermarket_id FROM price_alert_supermarkets ORDER BY alert_id, position")
            .fetch_all(&mut *conn)
            .await?;
    let mut by_alert: HashMap<i64, Vec<String>> = HashMap::new();
    for (alert_id, sid) in scopes {
        by_alert.entry(alert_id).or_default().push(sid);
    }
    Ok(rows
        .into_iter()
        .map(|(id, product_id, target_price, all)| Alert {
            id,
            product_id,
            target_price,
            supermarket_ids: if all { None } else { Some(by_alert.remove(&id).unwrap_or_default()) },
        })
        .collect())
}

async fn view(db: &Db, alert: Alert) -> SResult<AlertView> {
    let product = products::get(db, alert.product_id).await?;
    let best = latest_prices(db, product.id)
        .await?
        .into_iter()
        .find(|p| p.url_active && p.current_price.is_some() && alert.covers(&p.supermarket_id));
    Ok(AlertView { alert, product, best })
}

pub async fn set_alert(
    db: &Db,
    product_name: &str,
    target_price: i64,
    supermarket_ids: Option<&[String]>,
) -> SResult<(AlertView, bool)> {
    let (product, _) = products::resolve_product(db, product_name).await?;
    save(db, product, target_price, supermarket_ids).await
}

pub async fn set_alert_for_product(
    db: &Db,
    product_id: i64,
    target_price: i64,
    supermarket_ids: Option<&[String]>,
) -> SResult<(AlertView, bool)> {
    let product = products::get(db, product_id).await?;
    save(db, product, target_price, supermarket_ids).await
}

async fn save(
    db: &Db,
    product: Product,
    target_price: i64,
    supermarket_ids: Option<&[String]>,
) -> SResult<(AlertView, bool)> {
    if !(target_price > 0 && target_price <= MAX_TARGET) {
        let currency = crate::prefs::load(db).await?.currency;
        return validation(format!(
            "target_price must be a positive price in {}, e.g. {}.",
            currency.code(),
            currency.example()
        ));
    }
    let urls = products::urls_of(db, product.id).await?;
    let mut tracked: Vec<String> = urls.iter().map(|u| u.supermarket_id.clone()).collect();
    tracked.sort();
    let mut ids: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for raw in supermarket_ids.unwrap_or_default() {
        let id = if tracked.contains(raw) {
            raw.clone()
        } else {
            match supermarkets::find(db, raw).await?.filter(|m| tracked.contains(&m.id)) {
                Some(m) => m.id,
                None => {
                    if !missing.contains(raw) {
                        missing.push(raw.clone());
                    }
                    continue;
                }
            }
        };
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if !missing.is_empty() {
        let tracked_text = if tracked.is_empty() { "no supermarket yet".to_string() } else { tracked.join(", ") };
        return validation(format!(
            "'{}' is not tracked at: {}. It is tracked at: {tracked_text}.",
            product.name,
            missing.join(", ")
        ));
    }
    let now = now_ms();
    let mut tx = db::begin(db).await?;
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM price_alerts WHERE product_id = ?")
        .bind(product.id)
        .fetch_optional(&mut *tx)
        .await?;
    let created = existing.is_none();
    let alert_id: i64 = match existing {
        Some(id) => {
            sqlx::query("UPDATE price_alerts SET target_price = ?, all_supermarkets = ?, updated_at = ? WHERE id = ?")
                .bind(target_price)
                .bind(ids.is_empty())
                .bind(now)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            id
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO price_alerts (product_id, target_price, all_supermarkets, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(product.id)
            .bind(target_price)
            .bind(ids.is_empty())
            .bind(now)
            .bind(now)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    sqlx::query("DELETE FROM price_alert_supermarkets WHERE alert_id = ?").bind(alert_id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM price_alert_notified WHERE alert_id = ?").bind(alert_id).execute(&mut *tx).await?;
    for (pos, id) in ids.iter().enumerate() {
        sqlx::query("INSERT INTO price_alert_supermarkets (alert_id, supermarket_id, position) VALUES (?, ?, ?)")
            .bind(alert_id)
            .bind(id)
            .bind(pos as i64)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    tracing::info!(product = %product.name, target_price, "price alert saved");
    let alert = Alert {
        id: alert_id,
        product_id: product.id,
        target_price,
        supermarket_ids: if ids.is_empty() { None } else { Some(ids) },
    };
    Ok((view(db, alert).await?, created))
}

pub async fn list_alerts(db: &Db) -> SResult<Vec<AlertView>> {
    let mut conn = db.acquire().await?;
    let alerts = load_alerts(&mut conn, None).await?;
    drop(conn);
    let mut views = Vec::new();
    for a in alerts {
        views.push(view(db, a).await?);
    }
    views.sort_by(|a, b| a.product.name.cmp(&b.product.name));
    Ok(views)
}

pub async fn count(db: &Db) -> SResult<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM price_alerts").fetch_one(db).await?)
}

pub async fn delete_alert(db: &Db, product_name: &str) -> SResult<String> {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT a.id, p.name FROM price_alerts a JOIN products p ON p.id = a.product_id ORDER BY a.id")
            .fetch_all(db)
            .await?;
    let Some(best) = fuzzy_rank(product_name, rows, 1, 60.0).into_iter().next() else {
        return not_found(format!("No price alert matches '{product_name}'. Use get_price_alerts."));
    };
    delete_alert_by_id(db, best.key).await?;
    Ok(best.name)
}

pub async fn delete_alert_by_id(db: &Db, alert_id: i64) -> SResult<()> {
    let done = sqlx::query("DELETE FROM price_alerts WHERE id = ?").bind(alert_id).execute(db).await?;
    if done.rows_affected() == 0 {
        return Err(ServiceError::NotFound(format!("Price alert #{alert_id} not found.")));
    }
    tracing::info!(alert_id, "price alert deleted");
    Ok(())
}

pub async fn evaluate(db: &Db, saved: &[SavedPrice]) -> SResult<Vec<AlertHit>> {
    let mut by_product: BTreeMap<i64, Vec<&SavedPrice>> = BTreeMap::new();
    for p in saved {
        by_product.entry(p.product_id).or_default().push(p);
    }
    if by_product.is_empty() {
        return Ok(Vec::new());
    }
    let product_ids: Vec<i64> = by_product.keys().copied().collect();
    let mut tx = db::begin(db).await?;
    let alerts = load_alerts(&mut tx, Some(&product_ids)).await?;
    let mut hits = Vec::new();
    for alert in alerts {
        let mut notified: HashMap<String, i64> = sqlx::query_as::<_, (String, i64)>(
            "SELECT supermarket_id, price FROM price_alert_notified WHERE alert_id = ?",
        )
        .bind(alert.id)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
        let mut found: Vec<SavedPrice> = Vec::new();
        for p in by_product.get(&alert.product_id).into_iter().flatten() {
            if !alert.covers(&p.supermarket_id) {
                continue;
            }
            if p.price < alert.target_price {
                let previous = notified.get(&p.supermarket_id).copied();
                if previous.is_none_or(|prev| p.price < prev) {
                    found.push((*p).clone());
                    notified.insert(p.supermarket_id.clone(), p.price);
                }
            } else {
                notified.remove(&p.supermarket_id);
            }
        }
        sqlx::query("DELETE FROM price_alert_notified WHERE alert_id = ?").bind(alert.id).execute(&mut *tx).await?;
        for (sid, price) in &notified {
            sqlx::query("INSERT INTO price_alert_notified (alert_id, supermarket_id, price) VALUES (?, ?, ?)")
                .bind(alert.id)
                .bind(sid)
                .bind(price)
                .execute(&mut *tx)
                .await?;
        }
        if !found.is_empty() {
            found.sort_by_key(|p| p.price);
            tracing::info!(product = %found[0].product_name, target = alert.target_price, "price alert hit");
            hits.push(AlertHit {
                product_id: alert.product_id,
                product_name: found[0].product_name.clone(),
                target_price: alert.target_price,
                prices: found,
            });
        }
    }
    tx.commit().await?;
    Ok(hits)
}
