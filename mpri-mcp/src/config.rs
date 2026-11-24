// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use chrono_tz::Tz;

use crate::locale::{Currency, Lang};

pub const DEFAULT_BIND: &str = "127.0.0.1:8765";
pub const DB_FILE: &str = "mprimcp.db";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Stdio,
    Serve,
    Dashboard,
    Cli,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Stdio => "stdio",
            Mode::Serve => "serve",
            Mode::Dashboard => "dashboard",
            Mode::Cli => "cli",
        }
    }
}

#[derive(Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
    pub public_url: String,
    pub cron_enabled: bool,
    pub admin_user: Option<String>,
    pub admin_password: Option<String>,
    pub trust_proxy: bool,
    pub extra_redirect_uris: Vec<String>,
    pub telegram_api: String,
    pub import_from: Option<PathBuf>,
    pub currency: Option<Currency>,
    pub language: Option<Lang>,
    pub timezone: Option<Tz>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" | "" => Some(false),
        _ => None,
    }
}

fn env_bool(name: &str) -> Result<bool> {
    match env(name) {
        None => Ok(false),
        Some(v) => parse_bool(&v).ok_or_else(|| anyhow!("{name} must be true or false, got '{v}'")),
    }
}

pub fn default_data_dir() -> PathBuf {
    directories::BaseDirs::new().map(|d| d.data_dir().join("mprimcp")).unwrap_or_else(|| PathBuf::from("mprimcp-data"))
}

impl Config {
    pub fn from_env(bind_override: Option<&str>) -> Result<Self> {
        let _ = dotenvy::dotenv();
        let data_dir = env("MPRIMCP_DATA_DIR").map(PathBuf::from).unwrap_or_else(default_data_dir);
        let bind_raw = bind_override
            .map(str::to_string)
            .or_else(|| env("MPRIMCP_BIND"))
            .unwrap_or_else(|| DEFAULT_BIND.to_string());
        let bind: SocketAddr =
            bind_raw.parse().with_context(|| format!("MPRIMCP_BIND must be host:port, got '{bind_raw}'"))?;
        let public_url = env("MPRIMCP_PUBLIC_URL").map(|u| u.trim_end_matches('/').to_string()).unwrap_or_else(|| {
            if bind.ip().is_unspecified() {
                format!("http://localhost:{}", bind.port())
            } else {
                format!("http://{bind}")
            }
        });
        url::Url::parse(&public_url).with_context(|| format!("MPRIMCP_PUBLIC_URL is not a URL: '{public_url}'"))?;
        let extra_redirect_uris = env("MPRIMCP_OAUTH_REDIRECT_URIS")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        Ok(Self {
            data_dir,
            bind,
            public_url,
            cron_enabled: env_bool("MPRIMCP_CRON_ENABLED")?,
            admin_user: env("MPRIMCP_ADMIN_USER"),
            admin_password: std::env::var("MPRIMCP_ADMIN_PASSWORD").ok().filter(|v| !v.is_empty()),
            trust_proxy: env_bool("MPRIMCP_TRUST_PROXY")?,
            extra_redirect_uris,
            telegram_api: env("MPRIMCP_TELEGRAM_API").unwrap_or_else(|| "https://api.telegram.org".into()),
            import_from: env("MPRIMCP_IMPORT_FROM").map(PathBuf::from),
            currency: env("MPRIMCP_CURRENCY")
                .map(|v| Currency::parse(&v).ok_or_else(|| anyhow!("MPRIMCP_CURRENCY must be VND or USD, got '{v}'")))
                .transpose()?,
            language: env("MPRIMCP_LANGUAGE")
                .map(|v| Lang::parse(&v).ok_or_else(|| anyhow!("MPRIMCP_LANGUAGE must be en or vi, got '{v}'")))
                .transpose()?,
            timezone: env("MPRIMCP_TIMEZONE")
                .map(|v| {
                    v.parse::<Tz>().map_err(|_| {
                        anyhow!("MPRIMCP_TIMEZONE must be an IANA timezone such as America/New_York, got '{v}'")
                    })
                })
                .transpose()?,
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join(DB_FILE)
    }

    pub fn is_https(&self) -> bool {
        self.public_url.starts_with("https://")
    }

    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.public_url)
    }

    pub fn public_authority(&self) -> String {
        url::Url::parse(&self.public_url)
            .ok()
            .map(|u| match (u.host_str(), u.port()) {
                (Some(h), Some(p)) => format!("{h}:{p}"),
                (Some(h), None) => h.to_string(),
                _ => String::new(),
            })
            .unwrap_or_default()
    }

    pub fn public_host(&self) -> String {
        url::Url::parse(&self.public_url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default()
    }
}
