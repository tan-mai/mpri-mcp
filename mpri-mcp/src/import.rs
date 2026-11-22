// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::db::{self, Db};
use crate::text::{name_key, normalize_name};

#[derive(Debug, Clone, Default)]
pub struct TableStats {
    pub table: &'static str,
    pub source: u64,
    pub inserted: u64,
}

impl TableStats {
    pub fn skipped(&self) -> u64 {
        self.source - self.inserted
    }
}

#[derive(Debug, Default)]
pub struct ImportReport {
    pub tables: Vec<TableStats>,
    pub notes: Vec<String>,
}

fn ms(value: Option<String>) -> Option<i64> {
    let v = value?;
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(v) {
        return Some(dt.timestamp_millis());
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(v, fmt) {
            return Some(n.and_utc().timestamp_millis());
        }
    }
    None
}

fn req_ms(row: &SqliteRow, col: &str) -> i64 {
    ms(row.try_get::<Option<String>, _>(col).ok().flatten()).unwrap_or_else(|| Utc::now().timestamp_millis())
}

fn opt_ms(row: &SqliteRow, col: &str) -> Option<i64> {
    ms(row.try_get::<Option<String>, _>(col).ok().flatten())
}

fn text(row: &SqliteRow, col: &str) -> Option<String> {
    row.try_get::<Option<String>, _>(col).ok().flatten()
}

fn int(row: &SqliteRow, col: &str) -> Option<i64> {
    row.try_get::<Option<i64>, _>(col).ok().flatten()
}

fn flag(row: &SqliteRow, col: &str) -> Option<bool> {
    int(row, col).map(|v| v != 0)
}

fn json_scopes(raw: Option<String>) -> String {
    match raw.as_deref().map(serde_json::from_str::<Value>) {
        Some(Ok(Value::Array(a))) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
        Some(Ok(Value::String(s))) => s,
        _ => String::new(),
    }
}

async fn has_table(src: &SqlitePool, name: &str) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
        .bind(name)
        .fetch_one(src)
        .await?
        > 0)
}

async fn rows(src: &SqlitePool, table: &str, sql: &'static str) -> Result<Vec<SqliteRow>> {
    if !has_table(src, table).await? {
        return Ok(Vec::new());
    }
    Ok(sqlx::query(sql).fetch_all(src).await?)
}

struct Counter<'a> {
    stats: TableStats,
    conn: &'a mut SqliteConnection,
}

impl<'a> Counter<'a> {
    fn new(table: &'static str, conn: &'a mut SqliteConnection) -> Self {
        Self { stats: TableStats { table, ..Default::default() }, conn }
    }

    async fn run<'q>(&mut self, q: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>) -> Result<()> {
        self.stats.source += 1;
        self.stats.inserted += q.execute(&mut *self.conn).await?.rows_affected();
        Ok(())
    }
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub async fn import(target: &Db, from: &Path, secrets_dir: Option<&Path>) -> Result<ImportReport> {
    if !from.exists() {
        bail!("{} does not exist", from.display());
    }
    let wal = PathBuf::from(format!("{}-wal", from.display()));
    let has_wal = std::fs::metadata(&wal).map(|m| m.len() > 0).unwrap_or(false);
    let src = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(from).read_only(true).immutable(!has_wal))
        .await
        .with_context(|| format!("cannot open {}", from.display()))?;
    if !has_table(&src, "supermarkets").await? {
        bail!("{} is not an MPriMCP (Python) database: table 'supermarkets' is missing", from.display());
    }
    if has_table(&src, "supermarkets").await? {
        let cols: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('supermarkets')").fetch_all(&src).await?;
        if cols.iter().any(|c| c == "name_key") {
            bail!("{} is already a Rust MPriMCP database; copy the file instead of importing it", from.display());
        }
    }
    let mut report = ImportReport::default();
    let mut tx = db::begin(target).await?;

    let mut c = Counter::new("supermarkets", &mut tx);
    for r in rows(&src, "supermarkets", "SELECT * FROM supermarkets ORDER BY id").await? {
        let name = text(&r, "name").unwrap_or_default();
        c.run(
            sqlx::query(
                "INSERT OR IGNORE INTO supermarkets (id, name, name_key, base_url, search_url_template, cron_expression, \
                 cron_enabled, timezone, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(text(&r, "id"))
            .bind(&name)
            .bind(name_key(&name))
            .bind(text(&r, "base_url"))
            .bind(text(&r, "search_url_template"))
            .bind(text(&r, "cron_expression"))
            .bind(flag(&r, "cron_enabled").unwrap_or(false))
            .bind(text(&r, "timezone").unwrap_or_else(|| crate::clock::TIMEZONE.into()))
            .bind(req_ms(&r, "created_at"))
            .bind(req_ms(&r, "updated_at")),
        )
        .await?;
    }
    report.tables.push(c.stats);

    let mut c = Counter::new("products", &mut tx);
    for r in rows(&src, "products", "SELECT * FROM products ORDER BY id").await? {
        let name = text(&r, "name").unwrap_or_default();
        let created = req_ms(&r, "created_at");
        c.run(
            sqlx::query(
                "INSERT OR IGNORE INTO products (id, name, name_key, category, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(int(&r, "id"))
            .bind(&name)
            .bind(normalize_name(&name))
            .bind(text(&r, "category"))
            .bind(created)
            .bind(created),
        )
        .await?;
    }
    report.tables.push(c.stats);

    let mut c = Counter::new("product_urls", &mut tx);
    for r in rows(&src, "product_urls", "SELECT * FROM product_urls ORDER BY id").await? {
        c.run(
            sqlx::query(
                "INSERT OR IGNORE INTO product_urls (id, product_id, supermarket_id, url, is_active, in_stock, \
                 last_crawled_at, last_success_at, last_error, consecutive_failures, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(int(&r, "id"))
            .bind(int(&r, "product_id"))
            .bind(text(&r, "supermarket_id"))
            .bind(text(&r, "url"))
            .bind(flag(&r, "is_active").unwrap_or(true))
            .bind(flag(&r, "in_stock"))
            .bind(opt_ms(&r, "last_crawled_at"))
            .bind(opt_ms(&r, "last_success_at"))
            .bind(text(&r, "last_error"))
            .bind(int(&r, "consecutive_failures").unwrap_or(0))
            .bind(req_ms(&r, "created_at")),
        )
        .await?;
    }
    report.tables.push(c.stats);

    let mut c = Counter::new("price_records", &mut tx);
    for r in rows(&src, "price_records", "SELECT * FROM price_records ORDER BY id").await? {
        c.run(
            sqlx::query(
                "INSERT OR IGNORE INTO price_records (id, product_url_id, current_price, original_price, member_price, \
                 in_stock, source, needs_review, raw_text, crawled_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(int(&r, "id"))
            .bind(int(&r, "product_url_id"))
            .bind(int(&r, "current_price"))
            .bind(int(&r, "original_price"))
            .bind(int(&r, "member_price"))
            .bind(flag(&r, "in_stock"))
            .bind(text(&r, "source").unwrap_or_else(|| "agent".into()))
            .bind(flag(&r, "needs_review").unwrap_or(false))
            .bind(text(&r, "raw_text"))
            .bind(req_ms(&r, "crawled_at")),
        )
        .await?;
    }
    report.tables.push(c.stats);

    let mut c = Counter::new("crawl_runs", &mut tx);
    for r in rows(&src, "crawl_runs", "SELECT * FROM crawl_runs ORDER BY id").await? {
        c.run(
            sqlx::query(
                "INSERT OR IGNORE INTO crawl_runs (id, supermarket_id, trigger, status, started_at, finished_at, \
                 urls_total, urls_ok, urls_failed, error) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(int(&r, "id"))
            .bind(text(&r, "supermarket_id"))
            .bind(text(&r, "trigger"))
            .bind(text(&r, "status"))
            .bind(req_ms(&r, "started_at"))
            .bind(opt_ms(&r, "finished_at"))
            .bind(int(&r, "urls_total").unwrap_or(0))
            .bind(int(&r, "urls_ok").unwrap_or(0))
            .bind(int(&r, "urls_failed").unwrap_or(0))
            .bind(text(&r, "error")),
        )
        .await?;
    }
    report.tables.push(c.stats);

    let mut c = Counter::new("crawl_tasks", &mut tx);
    for r in rows(&src, "crawl_tasks", "SELECT * FROM crawl_tasks ORDER BY created_at").await? {
        c.run(
            sqlx::query(
                "INSERT OR IGNORE INTO crawl_tasks (id, product_url_id, supermarket_id, run_id, url, status, attempts, \
                 created_at, claimed_at, lease_expires_at, completed_at, last_error) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(text(&r, "id"))
            .bind(int(&r, "product_url_id"))
            .bind(text(&r, "supermarket_id"))
            .bind(int(&r, "run_id"))
            .bind(text(&r, "url"))
            .bind(text(&r, "status"))
            .bind(int(&r, "attempts").unwrap_or(0))
            .bind(req_ms(&r, "created_at"))
            .bind(opt_ms(&r, "claimed_at"))
            .bind(opt_ms(&r, "lease_expires_at"))
            .bind(opt_ms(&r, "completed_at"))
            .bind(text(&r, "last_error")),
        )
        .await?;
    }
    report.tables.push(c.stats);

    let alert_rows = rows(&src, "price_alerts", "SELECT * FROM price_alerts ORDER BY id").await?;
    let mut c = Counter::new("price_alerts", &mut tx);
    for r in &alert_rows {
        let scope: Option<Vec<String>> = text(r, "supermarket_ids")
            .and_then(|s| serde_json::from_str::<Option<Vec<String>>>(&s).ok())
            .flatten()
            .filter(|v| !v.is_empty());
        c.run(
            sqlx::query(
                "INSERT OR IGNORE INTO price_alerts (id, product_id, target_price, all_supermarkets, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(int(r, "id"))
            .bind(int(r, "product_id"))
            .bind(int(r, "target_price"))
            .bind(scope.is_none())
            .bind(req_ms(r, "created_at"))
            .bind(req_ms(r, "updated_at")),
        )
        .await?;
    }
    report.tables.push(c.stats);
    for r in &alert_rows {
        let id = int(r, "id");
        let scope: Vec<String> = text(r, "supermarket_ids")
            .and_then(|s| serde_json::from_str::<Option<Vec<String>>>(&s).ok())
            .flatten()
            .unwrap_or_default();
        for (pos, sid) in scope.iter().enumerate() {
            sqlx::query(
                "INSERT OR IGNORE INTO price_alert_supermarkets (alert_id, supermarket_id, position) \
                 SELECT ?, ?, ? WHERE EXISTS (SELECT 1 FROM supermarkets WHERE id = ?)",
            )
            .bind(id)
            .bind(sid)
            .bind(pos as i64)
            .bind(sid)
            .execute(&mut *tx)
            .await?;
        }
        let notified: serde_json::Map<String, Value> =
            text(r, "notified").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        for (sid, price) in notified {
            if let Some(p) = price.as_i64() {
                sqlx::query(
                    "INSERT OR IGNORE INTO price_alert_notified (alert_id, supermarket_id, price) \
                     SELECT ?, ?, ? WHERE EXISTS (SELECT 1 FROM supermarkets WHERE id = ?)",
                )
                .bind(id)
                .bind(&sid)
                .bind(p)
                .bind(&sid)
                .execute(&mut *tx)
                .await?;
            }
        }
    }

    let settings: Value = if has_table(&src, "app_settings").await? {
        sqlx::query_scalar::<_, String>("SELECT value FROM app_settings WHERE key = 'notifications'")
            .fetch_optional(&src)
            .await?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    sqlx::query(
        "INSERT INTO notification_settings (id, summary_enabled, last_summary_at) VALUES (1, ?1, ?2) \
         ON CONFLICT (id) DO UPDATE SET summary_enabled = ?1, last_summary_at = COALESCE(last_summary_at, ?2)",
    )
    .bind(settings["summary_enabled"].as_bool().unwrap_or(true))
    .bind(ms(settings["last_summary_at"].as_str().map(str::to_string)))
    .execute(&mut *tx)
    .await?;
    let secrets: PathBuf =
        secrets_dir.map(Path::to_path_buf).unwrap_or_else(|| from.parent().unwrap_or(Path::new(".")).join("secrets"));
    let read_secret = |name: &str| {
        std::fs::read_to_string(secrets.join(name)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    };
    let mut c = Counter::new("notification_channels", &mut tx);
    let now = Utc::now().timestamp_millis();
    for (channel, file, enabled_key, chat) in [
        ("telegram", "telegram_bot_token", "telegram_enabled", settings["telegram_chat_id"].as_str()),
        ("discord", "discord_webhook_url", "discord_enabled", None),
    ] {
        let Some(token) = read_secret(file) else { continue };
        c.run(
            sqlx::query(
                "INSERT OR IGNORE INTO notification_channels (channel, enabled, token, chat_id, updated_at) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(channel)
            .bind(settings[enabled_key].as_bool().unwrap_or(false))
            .bind(token)
            .bind(chat)
            .bind(now),
        )
        .await?;
    }
    if c.stats.source == 0 {
        report.notes.push(format!(
            "No Telegram/Discord secrets found in {} (pass --secrets-dir if they are elsewhere).",
            secrets.display()
        ));
    }
    report.tables.push(c.stats);

    let mut c = Counter::new("users", &mut tx);
    let mut incompatible = Vec::new();
    for r in rows(&src, "admin_users", "SELECT * FROM admin_users ORDER BY id").await? {
        let hash = text(&r, "password_hash").unwrap_or_default();
        let username = text(&r, "username").unwrap_or_default();
        if !hash.starts_with("$argon2") {
            incompatible.push(username.clone());
        }
        let changed = req_ms(&r, "password_changed_at");
        c.run(
            sqlx::query(
                "INSERT OR IGNORE INTO users (username, password_hash, session_version, created_at, updated_at, \
                 last_login_at, password_changed_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&username)
            .bind(&hash)
            .bind(int(&r, "session_version").unwrap_or(1))
            .bind(req_ms(&r, "created_at"))
            .bind(changed)
            .bind(opt_ms(&r, "last_login_at"))
            .bind(changed),
        )
        .await?;
    }
    report.tables.push(c.stats);
    if incompatible.is_empty() {
        report.notes.push("Account password hashes are argon2id and were kept: the old password still works.".into());
    } else {
        report.notes.push(format!(
            "Password hash not compatible for: {}. Run `mprimcp user set-password <username>`.",
            incompatible.join(", ")
        ));
    }

    let mut c = Counter::new("oauth_clients", &mut tx);
    for r in rows(&src, "oauth_clients", "SELECT * FROM oauth_clients").await? {
        let mut info: serde_json::Map<String, Value> =
            text(&r, "info").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        info.retain(|_, v| !v.is_null());
        info.remove("client_secret");
        c.run(
            sqlx::query(
                "INSERT OR IGNORE INTO oauth_clients (client_id, client_secret, client_name, metadata, created_at, \
                 last_used_at, revoked_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(text(&r, "client_id"))
            .bind(text(&r, "client_secret"))
            .bind(text(&r, "client_name"))
            .bind(Value::Object(info).to_string())
            .bind(req_ms(&r, "created_at"))
            .bind(opt_ms(&r, "last_used_at"))
            .bind(opt_ms(&r, "revoked_at")),
        )
        .await?;
    }
    report.tables.push(c.stats);

    let mut c = Counter::new("oauth_tokens", &mut tx);
    for r in rows(&src, "oauth_tokens", "SELECT * FROM oauth_tokens").await? {
        c.run(
            sqlx::query(
                "INSERT OR IGNORE INTO oauth_tokens (token_hash, kind, client_id, scopes, resource, subject, family_id, \
                 created_at, expires_at, used_at, revoked_at, last_used_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(text(&r, "token_hash"))
            .bind(text(&r, "kind"))
            .bind(text(&r, "client_id"))
            .bind(json_scopes(text(&r, "scopes")))
            .bind(text(&r, "resource"))
            .bind(text(&r, "subject"))
            .bind(text(&r, "family_id"))
            .bind(req_ms(&r, "created_at"))
            .bind(req_ms(&r, "expires_at"))
            .bind(opt_ms(&r, "used_at"))
            .bind(opt_ms(&r, "revoked_at"))
            .bind(opt_ms(&r, "last_used_at")),
        )
        .await?;
    }
    report.tables.push(c.stats);

    tx.commit().await?;
    src.close().await;
    Ok(report)
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub async fn auto_import(target: &Db, from: &Path) -> Result<Option<ImportReport>> {
    let markets: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM supermarkets").fetch_one(target).await?;
    let accounts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(target).await?;
    if markets > 0 || accounts > 0 {
        tracing::info!(from = %from.display(), "MPRIMCP_IMPORT_FROM ignored: the database already has data");
        return Ok(None);
    }
    if !from.exists() {
        tracing::warn!(from = %from.display(), "MPRIMCP_IMPORT_FROM file not found; starting with an empty database");
        return Ok(None);
    }
    let report = import(target, from, None).await?;
    for t in &report.tables {
        tracing::info!(table = t.table, source = t.source, inserted = t.inserted, "imported");
    }
    for n in &report.notes {
        tracing::info!("{n}");
    }
    Ok(Some(report))
}
