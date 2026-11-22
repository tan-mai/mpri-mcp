// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use sqlx::FromRow;

use crate::clock::{TIMEZONE, now_ms};
use crate::db::{self, Db};
use crate::error::{SResult, ServiceError, conflict, not_found, validation};
use crate::text::{collapse_spaces, name_key, normalize_base_url, normalize_name, slugify};

pub const SEARCH_PLACEHOLDER: &str = "{q}";
pub const SEARCH_NOTE: &str = "open base_url and use the site's search box";
const QUOTE_SAFE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

#[derive(Debug, Clone, FromRow)]
pub struct Supermarket {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub search_url_template: Option<String>,
    pub cron_expression: Option<String>,
    pub cron_enabled: bool,
    pub timezone: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct CrawlRun {
    pub id: i64,
    pub supermarket_id: String,
    pub trigger: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub urls_total: i64,
    pub urls_ok: i64,
    pub urls_failed: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub supermarket: Supermarket,
    pub url_count: i64,
    pub active_url_count: i64,
    pub failing_url_count: i64,
    pub last_run: Option<CrawlRun>,
    pub pending_tasks: i64,
    pub needs_review_count: i64,
}

#[derive(Debug, Clone)]
pub struct SearchUrl {
    pub supermarket: Supermarket,
    pub url: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Changes {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub search_url_template: Option<String>,
    pub timezone: Option<String>,
}

macro_rules! select_markets {
    () => {
        "SELECT id, name, base_url, search_url_template, cron_expression, cron_enabled, \
         timezone, created_at, updated_at FROM supermarkets"
    };
}

pub fn not_found_message(id: &str) -> String {
    format!("Supermarket '{id}' not found. Use get_supermarkets to list valid IDs.")
}

pub fn clean_search_url_template(value: Option<&str>) -> SResult<Option<String>> {
    let template = value.unwrap_or_default().trim().to_string();
    if template.is_empty() {
        return Ok(None);
    }
    if template.chars().count() > 500 {
        return validation("search_url_template is too long (max 500 characters).");
    }
    if template.matches(SEARCH_PLACEHOLDER).count() != 1 {
        return validation(
            "search_url_template must contain the placeholder {q} exactly once, e.g. \
             'https://cooponline.vn/search?router=productListing&query={q}'.",
        );
    }
    let probe = template.replace(SEARCH_PLACEHOLDER, "q");
    let ok = url::Url::parse(&probe)
        .map(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| h.contains('.')))
        .unwrap_or(false);
    if !ok {
        return validation("search_url_template must be a full http(s) URL.");
    }
    Ok(Some(template))
}

pub fn build_search_url(template: Option<&str>, keyword: &str) -> Option<String> {
    template.map(|t| t.replace(SEARCH_PLACEHOLDER, &utf8_percent_encode(keyword, QUOTE_SAFE).to_string()))
}

pub async fn get(db: &Db, id: &str) -> SResult<Option<Supermarket>> {
    Ok(sqlx::query_as::<_, Supermarket>(concat!(select_markets!(), " WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await?)
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub async fn find(db: &Db, id_or_name: &str) -> SResult<Option<Supermarket>> {
    let raw = id_or_name.trim();
    if let Some(m) = get(db, raw).await? {
        return Ok(Some(m));
    }
    let key = name_key(raw);
    if key.is_empty() {
        return Ok(None);
    }
    if let Some(m) = sqlx::query_as::<_, Supermarket>(concat!(select_markets!(), " WHERE name_key = ?"))
        .bind(&key)
        .fetch_optional(db)
        .await?
    {
        return Ok(Some(m));
    }
    let loose = normalize_name(raw);
    let mut matches: Vec<Supermarket> =
        list(db).await?.into_iter().filter(|m| normalize_name(&m.name) == loose).collect();
    Ok(if matches.len() == 1 { matches.pop() } else { None })
}

pub async fn resolve(db: &Db, id_or_name: &str) -> SResult<Supermarket> {
    match find(db, id_or_name).await? {
        Some(m) => Ok(m),
        None => not_found(not_found_message(id_or_name)),
    }
}

pub async fn list(db: &Db) -> SResult<Vec<Supermarket>> {
    Ok(sqlx::query_as::<_, Supermarket>(concat!(select_markets!(), " ORDER BY name")).fetch_all(db).await?)
}

pub async fn list_summaries(db: &Db) -> SResult<Vec<Summary>> {
    let markets = list(db).await?;
    let counts: HashMap<String, (i64, i64, i64)> = sqlx::query_as::<_, (String, i64, i64, i64)>(
        "SELECT supermarket_id, COUNT(*), \
                COALESCE(SUM(is_active), 0), \
                COALESCE(SUM(CASE WHEN is_active = 1 AND consecutive_failures > 0 THEN 1 ELSE 0 END), 0) \
         FROM product_urls GROUP BY supermarket_id",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|(id, a, b, c)| (id, (a, b, c)))
    .collect();
    let runs: HashMap<String, CrawlRun> = sqlx::query_as::<_, CrawlRun>(
        "SELECT * FROM crawl_runs WHERE id IN (SELECT MAX(id) FROM crawl_runs GROUP BY supermarket_id)",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|r| (r.supermarket_id.clone(), r))
    .collect();
    let pending: HashMap<String, i64> = sqlx::query_as::<_, (String, i64)>(
        "SELECT supermarket_id, COUNT(*) FROM crawl_tasks \
         WHERE status IN ('pending', 'claimed') GROUP BY supermarket_id",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .collect();
    let flagged: HashMap<String, i64> = sqlx::query_as::<_, (String, i64)>(
        "SELECT u.supermarket_id, COUNT(*) FROM price_records p \
         JOIN product_urls u ON u.id = p.product_url_id \
         WHERE p.needs_review = 1 GROUP BY u.supermarket_id",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .collect();
    Ok(markets
        .into_iter()
        .map(|m| {
            let (total, active, failing) = counts.get(&m.id).copied().unwrap_or((0, 0, 0));
            Summary {
                url_count: total,
                active_url_count: active,
                failing_url_count: failing,
                last_run: runs.get(&m.id).cloned(),
                pending_tasks: pending.get(&m.id).copied().unwrap_or(0),
                needs_review_count: flagged.get(&m.id).copied().unwrap_or(0),
                supermarket: m,
            }
        })
        .collect())
}

pub async fn search_urls(db: &Db, keyword: &str, supermarket_ids: Option<&[String]>) -> SResult<Vec<SearchUrl>> {
    let keyword = collapse_spaces(keyword);
    if keyword.is_empty() {
        return validation("keyword is required.");
    }
    if keyword.chars().count() > 200 {
        return validation("keyword is too long (max 200 characters).");
    }
    let mut markets = list(db).await?;
    if let Some(ids) = supermarket_ids.filter(|ids| !ids.is_empty()) {
        let mut wanted: Vec<&String> = Vec::new();
        for id in ids {
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }
        let mut chosen = Vec::new();
        let mut unknown = Vec::new();
        for id in wanted {
            match find(db, id).await? {
                Some(m) if !chosen.iter().any(|c: &Supermarket| c.id == m.id) => chosen.push(m),
                Some(_) => {}
                None => unknown.push(id.clone()),
            }
        }
        if !unknown.is_empty() {
            return not_found(format!(
                "Unknown supermarket ID(s): {}. Use get_supermarkets to list valid IDs.",
                unknown.join(", ")
            ));
        }
        markets = chosen;
    }
    Ok(markets
        .into_iter()
        .map(|m| {
            let url = build_search_url(m.search_url_template.as_deref(), &keyword);
            SearchUrl { supermarket: m, url }
        })
        .collect())
}

fn clean_name(name: &str) -> SResult<String> {
    let name = collapse_spaces(name);
    if name.is_empty() {
        return validation("Supermarket name is required.");
    }
    if name.chars().count() > 200 {
        return validation("Supermarket name is too long (max 200 characters).");
    }
    Ok(name)
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub async fn add(db: &Db, name: &str, base_url: &str, search_url_template: Option<&str>) -> SResult<Supermarket> {
    let name = clean_name(name)?;
    let base = normalize_base_url(base_url).map_err(ServiceError::Validation)?;
    let template = clean_search_url_template(search_url_template)?;
    let key = name_key(&name);
    let mut tx = db::begin(db).await?;
    let existing: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, name, name_key FROM supermarkets").fetch_all(&mut *tx).await?;
    if let Some((id, existing_name, _)) = existing.iter().find(|(_, _, k)| *k == key) {
        return conflict(format!("Supermarket '{existing_name}' already exists with ID '{id}'."));
    }
    let truncated: String = slugify(&name).chars().take(60).collect();
    let mut base_slug = truncated.trim_matches('-').to_string();
    if base_slug.is_empty() {
        base_slug = "market".into();
    }
    let mut slug = base_slug.clone();
    let mut n = 2;
    while existing.iter().any(|(id, _, _)| *id == slug) {
        slug = format!("{base_slug}-{n}");
        n += 1;
    }
    let now = now_ms();
    sqlx::query(
        "INSERT INTO supermarkets (id, name, name_key, base_url, search_url_template, cron_enabled, \
         timezone, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 0, ?, ?, ?)",
    )
    .bind(&slug)
    .bind(&name)
    .bind(&key)
    .bind(&base)
    .bind(&template)
    .bind(TIMEZONE)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(supermarket = %slug, "supermarket added");
    get(db, &slug).await?.ok_or_else(|| ServiceError::Internal("supermarket vanished".into()))
}

pub async fn update(db: &Db, id_or_name: &str, changes: Changes) -> SResult<Supermarket> {
    let market = resolve(db, id_or_name).await?;
    let mut name = market.name.clone();
    let mut base_url = market.base_url.clone();
    let mut template = market.search_url_template.clone();
    let mut timezone = market.timezone.clone();
    if let Some(n) = &changes.name {
        name = clean_name(n)?;
        let key = name_key(&name);
        let clash: Option<(String,)> = sqlx::query_as("SELECT id FROM supermarkets WHERE name_key = ? AND id != ?")
            .bind(&key)
            .bind(&market.id)
            .fetch_optional(db)
            .await?;
        if clash.is_some() {
            return conflict(format!("Another supermarket is already named '{name}'."));
        }
    }
    if let Some(b) = &changes.base_url {
        base_url = normalize_base_url(b).map_err(ServiceError::Validation)?;
    }
    if let Some(t) = &changes.search_url_template {
        template = clean_search_url_template(Some(t))?;
    }
    if let Some(tz) = &changes.timezone {
        if tz.parse::<chrono_tz::Tz>().is_err() {
            return validation(format!("Unknown timezone '{tz}'."));
        }
        timezone = tz.clone();
    }
    sqlx::query(
        "UPDATE supermarkets SET name = ?, name_key = ?, base_url = ?, search_url_template = ?, \
         timezone = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&name)
    .bind(name_key(&name))
    .bind(&base_url)
    .bind(&template)
    .bind(&timezone)
    .bind(now_ms())
    .bind(&market.id)
    .execute(db)
    .await?;
    get(db, &market.id).await?.ok_or_else(|| ServiceError::NotFound(not_found_message(id_or_name)))
}

pub async fn set_schedule_fields(
    db: &Db,
    id: &str,
    cron_expression: Option<&str>,
    cron_enabled: bool,
) -> SResult<Supermarket> {
    sqlx::query("UPDATE supermarkets SET cron_expression = ?, cron_enabled = ?, updated_at = ? WHERE id = ?")
        .bind(cron_expression)
        .bind(cron_enabled)
        .bind(now_ms())
        .bind(id)
        .execute(db)
        .await?;
    get(db, id).await?.ok_or_else(|| ServiceError::NotFound(not_found_message(id)))
}

pub async fn delete(db: &Db, id: &str) -> SResult<()> {
    let done = sqlx::query("DELETE FROM supermarkets WHERE id = ?").bind(id).execute(db).await?;
    if done.rows_affected() == 0 {
        return not_found(not_found_message(id));
    }
    tracing::info!(supermarket = %id, "supermarket deleted");
    Ok(())
}

pub async fn count(db: &Db) -> SResult<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM supermarkets").fetch_one(db).await?)
}
