// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use anyhow::{Result, bail};
use chrono_tz::Tz;

use crate::clock::DEFAULT_TIMEZONE;
use crate::config::Config;
use crate::db::Db;
use crate::error::{SResult, ServiceError, validation};
use crate::locale::{Currency, Lang};

pub const DEFAULT_CURRENCY: Currency = Currency::Vnd;
pub const DEFAULT_LANGUAGE: Lang = Lang::En;

#[derive(Clone, Copy, Debug)]
pub struct Prefs {
    pub currency: Currency,
    pub language: Lang,
    pub timezone: Tz,
}

impl Prefs {
    pub fn tz_name(&self) -> &'static str {
        self.timezone.name()
    }
}

impl Default for Prefs {
    fn default() -> Self {
        Self { currency: DEFAULT_CURRENCY, language: DEFAULT_LANGUAGE, timezone: chrono_tz::Asia::Ho_Chi_Minh }
    }
}

pub async fn load_or_default(db: &Db) -> Prefs {
    load(db).await.unwrap_or_default()
}

#[derive(Debug, Default, Clone)]
pub struct Changes {
    pub currency: Option<Currency>,
    pub language: Option<Lang>,
    pub timezone: Option<String>,
}

async fn get(db: &Db, key: &str) -> SResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT value FROM app_meta WHERE key = ?").bind(key).fetch_optional(db).await?)
}

async fn set(db: &Db, key: &str, value: &str) -> SResult<()> {
    sqlx::query("INSERT INTO app_meta (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = ?2")
        .bind(key)
        .bind(value)
        .execute(db)
        .await?;
    Ok(())
}

async fn has_prices(db: &Db) -> SResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM price_records)").fetch_one(db).await?)
}

pub fn parse_timezone(raw: &str) -> SResult<Tz> {
    raw.trim().parse::<Tz>().map_err(|_| {
        ServiceError::Validation(format!("Unknown timezone '{raw}'. Use an IANA name such as America/New_York."))
    })
}

pub async fn load(db: &Db) -> SResult<Prefs> {
    let currency = get(db, "currency").await?.and_then(|v| Currency::parse(&v)).unwrap_or(DEFAULT_CURRENCY);
    let language = get(db, "language").await?.and_then(|v| Lang::parse(&v)).unwrap_or(DEFAULT_LANGUAGE);
    let timezone = get(db, "timezone")
        .await?
        .and_then(|v| v.parse::<Tz>().ok())
        .unwrap_or_else(|| DEFAULT_TIMEZONE.parse().unwrap_or(chrono_tz::Asia::Ho_Chi_Minh));
    Ok(Prefs { currency, language, timezone })
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub async fn init(db: &Db, config: &Config) -> Result<Prefs> {
    let stored = get(db, "currency").await?.and_then(|v| Currency::parse(&v));
    match (config.currency, stored) {
        (Some(wanted), current) => {
            let current = current.unwrap_or(DEFAULT_CURRENCY);
            if wanted != current && has_prices(db).await? {
                bail!(
                    "MPRIMCP_CURRENCY={} but the database already holds prices in {}. Remove MPRIMCP_CURRENCY \
                     or use another data directory.",
                    wanted.code(),
                    current.code()
                );
            }
            set(db, "currency", wanted.code()).await?;
        }
        (None, None) => set(db, "currency", DEFAULT_CURRENCY.code()).await?,
        (None, Some(_)) => {}
    }
    match config.language {
        Some(l) => set(db, "language", l.code()).await?,
        None if get(db, "language").await?.is_none() => set(db, "language", DEFAULT_LANGUAGE.code()).await?,
        None => {}
    }
    match config.timezone {
        Some(tz) => {
            set(db, "timezone", tz.name()).await?;
            sqlx::query("UPDATE supermarkets SET timezone = ?").bind(tz.name()).execute(db).await?;
        }
        None if get(db, "timezone").await?.is_none() => set(db, "timezone", DEFAULT_TIMEZONE).await?,
        None => {}
    }
    Ok(load(db).await?)
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub async fn update(db: &Db, config: &Config, changes: Changes) -> SResult<Prefs> {
    let current = load(db).await?;
    if let Some(c) = changes.currency.filter(|c| *c != current.currency) {
        if config.currency.is_some() {
            return validation("The currency is fixed by MPRIMCP_CURRENCY on the server.");
        }
        if has_prices(db).await? {
            return validation(format!(
                "The currency cannot change: prices are already stored in {}.",
                current.currency.code()
            ));
        }
        set(db, "currency", c.code()).await?;
    }
    if let Some(l) = changes.language.filter(|l| *l != current.language) {
        if config.language.is_some() {
            return validation("The language is fixed by MPRIMCP_LANGUAGE on the server.");
        }
        set(db, "language", l.code()).await?;
    }
    if let Some(raw) = changes.timezone.as_deref().filter(|t| !t.trim().is_empty()) {
        let tz = parse_timezone(raw)?;
        if tz != current.timezone {
            if config.timezone.is_some() {
                return validation("The timezone is fixed by MPRIMCP_TIMEZONE on the server.");
            }
            set(db, "timezone", tz.name()).await?;
            sqlx::query("UPDATE supermarkets SET timezone = ?").bind(tz.name()).execute(db).await?;
        }
    }
    let updated = load(db).await?;
    tracing::info!(
        currency = updated.currency.code(),
        language = updated.language.code(),
        timezone = updated.tz_name(),
        "preferences saved"
    );
    Ok(updated)
}
