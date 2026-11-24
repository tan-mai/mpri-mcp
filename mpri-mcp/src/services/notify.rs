// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use sqlx::FromRow;
use tokio_util::task::TaskTracker;

use crate::clock::now_ms;
use crate::db::Db;
use crate::error::{SResult, ServiceError, validation};
use crate::services::alerts::{self, AlertHit, SavedPrice};
use crate::services::notify_messages::{
    Line, alert_lines, render_discord, render_telegram, split_messages, summary_lines, test_lines,
};
use crate::text::mask_secret;

pub const CHANNELS: [&str; 2] = ["telegram", "discord"];
const TELEGRAM_LIMIT: usize = 4096;
const DISCORD_LIMIT: usize = 2000;
const DISCORD_PREFIXES: [&str; 4] = [
    "https://discord.com/api/webhooks/",
    "https://discordapp.com/api/webhooks/",
    "https://ptb.discord.com/api/webhooks/",
    "https://canary.discord.com/api/webhooks/",
];
const BACKOFF: [u64; 3] = [2, 5, 15];

#[derive(Debug, Clone, FromRow)]
pub struct ChannelConfig {
    pub channel: String,
    pub enabled: bool,
    pub token: String,
    pub chat_id: Option<String>,
    pub last_test_ok: Option<bool>,
    pub last_test_at: Option<i64>,
    pub last_test_error: Option<String>,
    pub updated_at: i64,
}

impl ChannelConfig {
    pub fn target_masked(&self) -> String {
        mask_secret(&self.token)
    }

    pub fn is_active(&self) -> bool {
        self.enabled && !self.token.is_empty() && (self.channel != "telegram" || self.chat_id.is_some())
    }
}

#[derive(Debug, Clone)]
pub struct NotifySettings {
    pub summary_enabled: bool,
    pub last_summary_at: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct AfterSubmit {
    pub alerts: Vec<AlertHit>,
    pub channels: Vec<String>,
    pub summary_sent_to: Vec<String>,
}

pub enum TokenChange {
    Keep,
    Set(String),
}

#[derive(Debug, Clone)]
pub struct SetChannelResult {
    pub channel: String,
    pub removed: bool,
    pub target_masked: Option<String>,
    pub enabled: bool,
    pub test_ok: bool,
    pub error: Option<String>,
}

pub fn validate_channel(channel: &str) -> SResult<String> {
    let c = channel.trim().to_lowercase();
    if CHANNELS.contains(&c.as_str()) {
        Ok(c)
    } else {
        validation(format!("Unknown channel '{channel}': use telegram or discord."))
    }
}

pub fn validate_telegram_token(token: &str) -> SResult<String> {
    let token = token.trim();
    let ok = token.split_once(':').is_some_and(|(id, rest)| {
        id.len() >= 5
            && id.bytes().all(|b| b.is_ascii_digit())
            && rest.len() >= 30
            && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    });
    if ok {
        Ok(token.to_string())
    } else {
        validation("That does not look like a Telegram bot token (from @BotFather, e.g. '123456789:AA…').")
    }
}

pub fn validate_discord_webhook(url: &str) -> SResult<String> {
    let url = url.trim();
    if DISCORD_PREFIXES.iter().any(|p| url.starts_with(p)) {
        Ok(url.to_string())
    } else {
        validation(
            "Paste the Discord webhook URL (Channel settings → Integrations → Webhooks → Copy Webhook URL); \
             it starts with https://discord.com/api/webhooks/.",
        )
    }
}

pub fn validate_chat_id(chat_id: &str) -> SResult<String> {
    let c = chat_id.trim();
    let numeric = {
        let digits = c.strip_prefix('-').unwrap_or(c);
        !digits.is_empty() && digits.len() <= 20 && digits.bytes().all(|b| b.is_ascii_digit())
    };
    let handle = c.strip_prefix('@').is_some_and(|h| {
        (5..=32).contains(&h.len())
            && h.starts_with(|ch: char| ch.is_ascii_alphabetic())
            && h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    });
    if numeric || handle {
        Ok(c.to_string())
    } else {
        validation("Chat ID is a number (e.g. 123456789 or -100…) or @channelname.")
    }
}

pub async fn get_channel(db: &Db, channel: &str) -> SResult<Option<ChannelConfig>> {
    Ok(sqlx::query_as("SELECT * FROM notification_channels WHERE channel = ?").bind(channel).fetch_optional(db).await?)
}

pub async fn list_channels(db: &Db) -> SResult<Vec<ChannelConfig>> {
    Ok(sqlx::query_as("SELECT * FROM notification_channels ORDER BY channel DESC").fetch_all(db).await?)
}

pub async fn get_settings(db: &Db) -> SResult<NotifySettings> {
    let row: Option<(bool, Option<i64>)> =
        sqlx::query_as("SELECT summary_enabled, last_summary_at FROM notification_settings WHERE id = 1")
            .fetch_optional(db)
            .await?;
    let (summary_enabled, last_summary_at) = row.unwrap_or((true, None));
    Ok(NotifySettings { summary_enabled, last_summary_at })
}

pub async fn set_summary_enabled(db: &Db, enabled: bool) -> SResult<()> {
    sqlx::query(
        "INSERT INTO notification_settings (id, summary_enabled) VALUES (1, ?1) \
         ON CONFLICT (id) DO UPDATE SET summary_enabled = ?1",
    )
    .bind(enabled)
    .execute(db)
    .await?;
    Ok(())
}

async fn set_last_summary(db: &Db, at: i64) -> SResult<()> {
    sqlx::query(
        "INSERT INTO notification_settings (id, last_summary_at) VALUES (1, ?1) \
         ON CONFLICT (id) DO UPDATE SET last_summary_at = ?1",
    )
    .bind(at)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Clone)]
pub struct Notifier {
    db: Db,
    http: reqwest::Client,
    telegram_api: Arc<String>,
    tracker: TaskTracker,
    backoff: Arc<Vec<Duration>>,
}

enum SendError {
    Retry(f64, String),
    Transient(String),
    Fatal(String),
}

fn json_body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

impl Notifier {
    pub fn new(db: Db, telegram_api: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            db,
            http,
            telegram_api: Arc::new(telegram_api.trim_end_matches('/').to_string()),
            tracker: TaskTracker::new(),
            backoff: Arc::new(BACKOFF.iter().map(|s| Duration::from_secs(*s)).collect()),
        }
    }

    pub fn with_backoff(mut self, backoff: Vec<Duration>) -> Self {
        self.backoff = Arc::new(backoff);
        self
    }

    pub async fn active_channels(&self) -> SResult<Vec<ChannelConfig>> {
        Ok(list_channels(&self.db).await?.into_iter().filter(ChannelConfig::is_active).collect())
    }

    // TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
    pub async fn set_channel(
        &self,
        channel: &str,
        token: TokenChange,
        chat_id: Option<&str>,
        enabled: bool,
    ) -> SResult<SetChannelResult> {
        let channel = validate_channel(channel)?;
        let existing = get_channel(&self.db, &channel).await?;
        let token = match token {
            TokenChange::Set(t) if t.trim().is_empty() => {
                sqlx::query("DELETE FROM notification_channels WHERE channel = ?")
                    .bind(&channel)
                    .execute(&self.db)
                    .await?;
                tracing::info!(%channel, "notification channel removed");
                return Ok(SetChannelResult {
                    channel,
                    removed: true,
                    target_masked: None,
                    enabled: false,
                    test_ok: false,
                    error: None,
                });
            }
            TokenChange::Set(t) if channel == "telegram" => validate_telegram_token(&t)?,
            TokenChange::Set(t) => validate_discord_webhook(&t)?,
            TokenChange::Keep => match &existing {
                Some(e) => e.token.clone(),
                None => {
                    return validation(if channel == "telegram" {
                        "Enter the Telegram bot token first."
                    } else {
                        "Enter the Discord webhook URL first."
                    });
                }
            },
        };
        let chat_id = match chat_id.map(str::trim).filter(|c| !c.is_empty()) {
            Some(c) if channel == "telegram" => Some(validate_chat_id(c)?),
            _ if channel == "telegram" => return validation("chat_id is required for Telegram."),
            _ => None,
        };
        sqlx::query(
            "INSERT INTO notification_channels (channel, enabled, token, chat_id, updated_at) VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT (channel) DO UPDATE SET enabled = ?2, token = ?3, chat_id = ?4, updated_at = ?5",
        )
        .bind(&channel)
        .bind(enabled)
        .bind(&token)
        .bind(&chat_id)
        .bind(now_ms())
        .execute(&self.db)
        .await?;
        tracing::info!(%channel, enabled, "notification channel saved");
        let test = self.send_test(&channel).await;
        Ok(SetChannelResult {
            channel,
            removed: false,
            target_masked: Some(mask_secret(&token)),
            enabled,
            test_ok: test.is_ok(),
            error: test.err(),
        })
    }

    pub async fn send_test(&self, channel: &str) -> Result<(), String> {
        let config = get_channel(&self.db, channel)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{channel} is not set up yet."))?;
        let lang = crate::prefs::load(&self.db).await.map(|p| p.language).unwrap_or(crate::prefs::DEFAULT_LANGUAGE);
        let result = self.deliver(&config, "test", &test_lines(lang)).await;
        let error = result.as_ref().err().cloned();
        let _ = sqlx::query(
            "UPDATE notification_channels SET last_test_ok = ?, last_test_at = ?, last_test_error = ? WHERE channel = ?",
        )
        .bind(result.is_ok())
        .bind(now_ms())
        .bind(&error)
        .bind(channel)
        .execute(&self.db)
        .await;
        result
    }

    pub async fn notify(&self, kind: &'static str, lines: Vec<Line>) -> SResult<Vec<String>> {
        let channels = self.active_channels().await?;
        let names = channels.iter().map(|c| c.channel.clone()).collect();
        for config in channels {
            let this = self.clone();
            let lines = lines.clone();
            self.tracker.spawn(async move {
                let _ = this.deliver(&config, kind, &lines).await;
            });
        }
        Ok(names)
    }

    pub async fn after_submit(&self, saved: &[SavedPrice], queue_empty: bool, accepted: i64) -> AfterSubmit {
        let mut out = AfterSubmit::default();
        let result: SResult<()> = async {
            out.channels = self.active_channels().await?.into_iter().map(|c| c.channel).collect();
            let prefs = crate::prefs::load(&self.db).await?;
            out.alerts = alerts::evaluate(&self.db, saved).await?;
            for hit in &out.alerts {
                self.notify("alert", alert_lines(hit, &prefs)).await?;
            }
            if queue_empty && accepted > 0 {
                let settings = get_settings(&self.db).await?;
                let now = now_ms();
                if settings.summary_enabled {
                    let lines = summary_lines(&self.db, settings.last_summary_at, now, &prefs).await?;
                    out.summary_sent_to = self.notify("summary", lines).await?;
                }
                set_last_summary(&self.db, now).await?;
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            tracing::error!(error = %e, "notifications after submit failed");
        }
        out
    }

    pub async fn shutdown(&self, grace: Duration) {
        self.tracker.close();
        let _ = tokio::time::timeout(grace, self.tracker.wait()).await;
    }

    fn scrub(&self, config: &ChannelConfig, text: &str) -> String {
        let cleaned = text.replace(&config.token, "***");
        cleaned.chars().take(500).collect()
    }

    async fn deliver(&self, config: &ChannelConfig, kind: &str, lines: &[Line]) -> Result<(), String> {
        let texts = if config.channel == "telegram" {
            split_messages(lines, render_telegram, TELEGRAM_LIMIT)
        } else {
            split_messages(lines, render_discord, DISCORD_LIMIT)
        };
        let mut result = Ok(());
        for text in texts {
            if let Err(e) = self.send_with_retry(config, &text).await {
                result = Err(self.scrub(config, &e));
                break;
            }
        }
        match &result {
            Ok(()) => tracing::info!(kind, channel = %config.channel, "notification sent"),
            Err(e) => tracing::warn!(kind, channel = %config.channel, error = %e, "notification failed"),
        }
        result
    }

    async fn send_with_retry(&self, config: &ChannelConfig, text: &str) -> Result<(), String> {
        let attempts = self.backoff.len() + 1;
        for attempt in 0..attempts {
            let wait = match self.post(config, text).await {
                Ok(()) => return Ok(()),
                Err(SendError::Fatal(e)) => return Err(e),
                Err(SendError::Retry(seconds, e)) => {
                    if attempt + 1 == attempts {
                        return Err(e);
                    }
                    Duration::from_secs_f64(seconds.clamp(0.0, 60.0))
                }
                Err(SendError::Transient(e)) => {
                    if attempt + 1 == attempts {
                        return Err(e);
                    }
                    self.backoff[attempt]
                }
            };
            tokio::time::sleep(wait).await;
        }
        Err("notification failed".into())
    }

    async fn post(&self, config: &ChannelConfig, text: &str) -> Result<(), SendError> {
        let transport = |e: reqwest::Error| SendError::Transient(e.without_url().to_string());
        if config.channel == "telegram" {
            let url = format!("{}/bot{}/sendMessage", self.telegram_api, config.token);
            let resp = self
                .http
                .post(url)
                .json(&json!({
                    "chat_id": config.chat_id,
                    "text": text,
                    "parse_mode": "HTML",
                    "disable_web_page_preview": true,
                }))
                .send()
                .await
                .map_err(transport)?;
            let status = resp.status().as_u16();
            let data = json_body(&resp.text().await.unwrap_or_default());
            if status == 429 {
                let retry = data["parameters"]["retry_after"].as_f64().unwrap_or(5.0);
                return Err(SendError::Retry(retry, "Telegram rate limit".into()));
            }
            if status >= 500 {
                return Err(SendError::Transient(format!("Telegram HTTP {status}")));
            }
            if status >= 400 || data["ok"].as_bool() != Some(true) {
                let description =
                    data["description"].as_str().map(str::to_string).unwrap_or_else(|| status.to_string());
                return Err(SendError::Fatal(format!("Telegram: {description}")));
            }
            Ok(())
        } else {
            let resp = self
                .http
                .post(&config.token)
                .json(&json!({"content": text, "allowed_mentions": {"parse": []}}))
                .send()
                .await
                .map_err(transport)?;
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            if status == 429 {
                let retry = json_body(&body)["retry_after"].as_f64().unwrap_or(5.0);
                return Err(SendError::Retry(retry, "Discord rate limit".into()));
            }
            if status >= 500 {
                return Err(SendError::Transient(format!("Discord HTTP {status}")));
            }
            if status >= 400 {
                let message = json_body(&body)["message"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| body.chars().take(200).collect());
                return Err(SendError::Fatal(format!("Discord HTTP {status}: {message}")));
            }
            Ok(())
        }
    }
}

impl From<SendError> for ServiceError {
    fn from(e: SendError) -> Self {
        match e {
            SendError::Retry(_, m) | SendError::Transient(m) | SendError::Fatal(m) => ServiceError::Internal(m),
        }
    }
}
