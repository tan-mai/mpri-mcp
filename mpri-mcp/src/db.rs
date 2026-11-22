// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Sqlite, Transaction};

pub type Db = SqlitePool;
pub type Tx = Transaction<'static, Sqlite>;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn open(path: &Path) -> Result<Db> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create data directory {}", parent.display()))?;
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(15))
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .with_context(|| format!("cannot open database {}", path.display()))?;
    MIGRATOR.run(&pool).await.context("database migration failed")?;
    Ok(pool)
}

pub async fn begin(db: &Db) -> sqlx::Result<Tx> {
    db.begin_with("BEGIN IMMEDIATE").await
}

pub fn json_ids<T: serde::Serialize>(ids: &[T]) -> String {
    serde_json::to_string(ids).unwrap_or_else(|_| "[]".into())
}
