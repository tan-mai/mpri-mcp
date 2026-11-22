// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use sqlx::FromRow;

use crate::clock::now_ms;
use crate::db::{self, Db};
use crate::error::{SResult, ServiceError, conflict, not_found, validation};
use crate::services::supermarkets::{self, Supermarket};
use crate::text::{FuzzyMatch, collapse_spaces, fuzzy_rank, normalize_name, same_site, validate_page_url};

#[derive(Debug, Clone, FromRow)]
pub struct Product {
    pub id: i64,
    pub name: String,
    pub category: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct ProductUrl {
    pub id: i64,
    pub product_id: i64,
    pub supermarket_id: String,
    pub supermarket_name: String,
    pub url: String,
    pub is_active: bool,
    pub in_stock: Option<bool>,
    pub last_crawled_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
    pub consecutive_failures: i64,
}

#[macro_export]
macro_rules! select_urls {
    () => {
        "SELECT u.id, u.product_id, u.supermarket_id, s.name AS supermarket_name, u.url, \
         u.is_active, u.in_stock, u.last_crawled_at, u.last_success_at, u.last_error, u.consecutive_failures \
         FROM product_urls u JOIN supermarkets s ON s.id = u.supermarket_id"
    };
}

#[derive(Debug, Clone)]
pub struct UrlInput {
    pub supermarket_id: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct UrlResult {
    pub supermarket_id: String,
    pub url: String,
    pub status: &'static str,
    pub message: Option<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AddProductResult {
    pub product: Product,
    pub created: bool,
    pub urls: Vec<UrlResult>,
}

#[derive(Debug, Clone)]
pub struct ProductListItem {
    pub product: Product,
    pub url_count: i64,
}

fn clean_name(name: &str) -> SResult<(String, String)> {
    let name = collapse_spaces(name);
    if name.is_empty() {
        return validation("Product name is required.");
    }
    if name.chars().count() > 300 {
        return validation("Product name is too long (max 300 characters).");
    }
    let key = normalize_name(&name);
    if key.is_empty() {
        return validation("Product name must contain letters or digits.");
    }
    Ok((name, key))
}

fn clean_category(category: Option<&str>) -> Option<String> {
    category.map(str::trim).filter(|c| !c.is_empty()).map(str::to_string)
}

pub async fn add_product_with_urls(
    db: &Db,
    name: &str,
    category: Option<&str>,
    urls: &[UrlInput],
) -> SResult<AddProductResult> {
    let (name, key) = clean_name(name)?;
    let category = clean_category(category);
    let mut resolved: Vec<(UrlInput, Option<Supermarket>)> = Vec::new();
    for item in urls {
        resolved.push((item.clone(), supermarkets::find(db, &item.supermarket_id).await?));
    }
    let now = now_ms();
    let mut tx = db::begin(db).await?;
    let existing: Option<Product> =
        sqlx::query_as("SELECT id, name, category, created_at FROM products WHERE name_key = ?")
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await?;
    let created = existing.is_none();
    let product_id = match &existing {
        Some(p) => {
            if category.is_some() && p.category.is_none() {
                sqlx::query("UPDATE products SET category = ?, updated_at = ? WHERE id = ?")
                    .bind(&category)
                    .bind(now)
                    .bind(p.id)
                    .execute(&mut *tx)
                    .await?;
            }
            p.id
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO products (name, name_key, category, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(&name)
            .bind(&key)
            .bind(&category)
            .bind(now)
            .bind(now)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    let mut current: HashMap<String, (i64, String, bool)> = sqlx::query_as::<_, (i64, String, String, bool)>(
        "SELECT id, supermarket_id, url, is_active FROM product_urls WHERE product_id = ?",
    )
    .bind(product_id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|(id, sid, url, active)| (sid, (id, url, active)))
    .collect();

    let mut results = Vec::new();
    for (item, market) in resolved {
        let Some(market) = market else {
            results.push(UrlResult {
                supermarket_id: item.supermarket_id.clone(),
                url: item.url.clone(),
                status: "error",
                message: Some(format!("Unknown supermarket_id '{}'.", item.supermarket_id)),
                warning: None,
            });
            continue;
        };
        let url = match validate_page_url(&item.url) {
            Ok(u) => u,
            Err(e) => {
                results.push(UrlResult {
                    supermarket_id: item.supermarket_id.clone(),
                    url: item.url.clone(),
                    status: "error",
                    message: Some(e),
                    warning: None,
                });
                continue;
            }
        };
        let warning = (!same_site(&url, &market.base_url))
            .then(|| format!("URL host does not match {} ({}).", market.name, market.base_url));
        let status = match current.get(&market.id) {
            None => {
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO product_urls (product_id, supermarket_id, url, is_active, created_at) \
                     VALUES (?, ?, ?, 1, ?) RETURNING id",
                )
                .bind(product_id)
                .bind(&market.id)
                .bind(&url)
                .bind(now)
                .fetch_one(&mut *tx)
                .await?;
                current.insert(market.id.clone(), (id, url.clone(), true));
                "added"
            }
            Some((_, existing_url, true)) if *existing_url == url => "unchanged",
            Some((id, _, _)) => {
                let id = *id;
                sqlx::query(
                    "UPDATE product_urls SET url = ?, is_active = 1, consecutive_failures = 0, \
                     last_error = NULL WHERE id = ?",
                )
                .bind(&url)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                current.insert(market.id.clone(), (id, url.clone(), true));
                "updated"
            }
        };
        results.push(UrlResult { supermarket_id: market.id, url, status, message: None, warning });
    }
    tx.commit().await?;
    let product = get(db, product_id).await?;
    Ok(AddProductResult { product, created, urls: results })
}

pub async fn get(db: &Db, id: i64) -> SResult<Product> {
    let found: Option<Product> = sqlx::query_as("SELECT id, name, category, created_at FROM products WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?;
    found.ok_or_else(|| ServiceError::NotFound(format!("Product #{id} not found.")))
}

pub async fn urls_of(db: &Db, product_id: i64) -> SResult<Vec<ProductUrl>> {
    Ok(sqlx::query_as::<_, ProductUrl>(concat!(select_urls!(), " WHERE u.product_id = ? ORDER BY s.name"))
        .bind(product_id)
        .fetch_all(db)
        .await?)
}

pub async fn find_products(db: &Db, query: &str, limit: usize) -> SResult<Vec<FuzzyMatch<i64>>> {
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM products ORDER BY id").fetch_all(db).await?;
    Ok(fuzzy_rank(query, rows, limit, 60.0))
}

pub async fn resolve_product(db: &Db, query: &str) -> SResult<(Product, Vec<FuzzyMatch<i64>>)> {
    let matches = find_products(db, query, 5).await?;
    let Some(best) = matches.first().cloned() else {
        return not_found(format!("No tracked product matches '{query}'. Add it first with add_product_with_urls."));
    };
    let alternatives = matches[1..].iter().filter(|m| best.score - m.score <= 5.0).cloned().collect();
    Ok((get(db, best.key).await?, alternatives))
}

pub async fn list(db: &Db, query: Option<&str>) -> SResult<Vec<ProductListItem>> {
    let rows: Vec<(i64, String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT p.id, p.name, p.category, p.created_at, COUNT(u.id) FROM products p \
         LEFT JOIN product_urls u ON u.product_id = p.id GROUP BY p.id ORDER BY p.name",
    )
    .fetch_all(db)
    .await?;
    let items: Vec<ProductListItem> = rows
        .into_iter()
        .map(|(id, name, category, created_at, n)| ProductListItem {
            product: Product { id, name, category, created_at },
            url_count: n,
        })
        .collect();
    match query.map(str::trim).filter(|q| !q.is_empty()) {
        None => Ok(items),
        Some(q) => {
            let ranked = fuzzy_rank(q, items.iter().map(|i| (i.product.id, i.product.name.clone())), 200, 60.0);
            let order: HashMap<i64, usize> = ranked.iter().enumerate().map(|(n, m)| (m.key, n)).collect();
            let mut filtered: Vec<ProductListItem> =
                items.into_iter().filter(|i| order.contains_key(&i.product.id)).collect();
            filtered.sort_by_key(|i| order[&i.product.id]);
            Ok(filtered)
        }
    }
}

pub async fn count(db: &Db) -> SResult<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM products").fetch_one(db).await?)
}

pub async fn count_without_urls(db: &Db) -> SResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM products p WHERE NOT EXISTS \
         (SELECT 1 FROM product_urls u WHERE u.product_id = p.id)",
    )
    .fetch_one(db)
    .await?)
}

pub async fn update(db: &Db, id: i64, name: &str, category: Option<&str>) -> SResult<Product> {
    let (name, key) = clean_name(name)?;
    get(db, id).await?;
    let clash: Option<(String,)> = sqlx::query_as("SELECT name FROM products WHERE name_key = ? AND id != ?")
        .bind(&key)
        .bind(id)
        .fetch_optional(db)
        .await?;
    if let Some((other,)) = clash {
        return conflict(format!("Another product is already named '{other}'."));
    }
    sqlx::query("UPDATE products SET name = ?, name_key = ?, category = ?, updated_at = ? WHERE id = ?")
        .bind(&name)
        .bind(&key)
        .bind(clean_category(category))
        .bind(now_ms())
        .bind(id)
        .execute(db)
        .await?;
    get(db, id).await
}

pub async fn delete(db: &Db, id: i64) -> SResult<()> {
    let done = sqlx::query("DELETE FROM products WHERE id = ?").bind(id).execute(db).await?;
    if done.rows_affected() == 0 {
        return not_found(format!("Product #{id} not found."));
    }
    Ok(())
}

pub async fn get_url(db: &Db, url_id: i64) -> SResult<ProductUrl> {
    let found: Option<ProductUrl> =
        sqlx::query_as(concat!(select_urls!(), " WHERE u.id = ?")).bind(url_id).fetch_optional(db).await?;
    found.ok_or_else(|| ServiceError::NotFound(format!("URL #{url_id} not found.")))
}

pub async fn update_url(db: &Db, url_id: i64, url: Option<&str>, is_active: Option<bool>) -> SResult<ProductUrl> {
    let row = get_url(db, url_id).await?;
    if let Some(u) = url {
        let new_url = validate_page_url(u).map_err(ServiceError::Validation)?;
        if new_url != row.url {
            sqlx::query("UPDATE product_urls SET url = ?, consecutive_failures = 0, last_error = NULL WHERE id = ?")
                .bind(&new_url)
                .bind(url_id)
                .execute(db)
                .await?;
        }
    }
    if let Some(active) = is_active {
        sqlx::query("UPDATE product_urls SET is_active = ? WHERE id = ?").bind(active).bind(url_id).execute(db).await?;
    }
    get_url(db, url_id).await
}

pub async fn delete_url(db: &Db, url_id: i64) -> SResult<()> {
    let done = sqlx::query("DELETE FROM product_urls WHERE id = ?").bind(url_id).execute(db).await?;
    if done.rows_affected() == 0 {
        return not_found(format!("URL #{url_id} not found."));
    }
    Ok(())
}
