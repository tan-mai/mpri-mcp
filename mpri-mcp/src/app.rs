// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::sync::Arc;

use anyhow::Result;

use crate::config::{Config, Mode};
use crate::db::{self, Db};
use crate::services::notify::Notifier;

#[derive(Clone)]
pub struct App {
    pub db: Db,
    pub config: Arc<Config>,
    pub mode: Mode,
    pub notifier: Notifier,
}

impl App {
    pub async fn open(config: Config, mode: Mode) -> Result<Self> {
        let db = db::open(&config.db_path()).await?;
        Ok(Self::new(db, config, mode))
    }

    pub fn new(db: Db, config: Config, mode: Mode) -> Self {
        let notifier = Notifier::new(db.clone(), &config.telegram_api);
        Self { db, config: Arc::new(config), mode, notifier }
    }

    pub fn server_cron(&self) -> bool {
        self.mode == Mode::Serve && self.config.cron_enabled
    }
}
