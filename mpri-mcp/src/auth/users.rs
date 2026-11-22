// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::io::{IsTerminal, Write};

use anyhow::{Result, anyhow, bail};
use sqlx::FromRow;

use crate::auth::passwords::{hash_password, validate_new_password, validate_username, verify_password};
use crate::clock::now_ms;
use crate::config::Config;
use crate::db::Db;
use crate::error::{SResult, ServiceError};

#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub session_version: i64,
    pub last_login_at: Option<i64>,
}

const SELECT: &str = "SELECT id, username, password_hash, session_version, last_login_at FROM users";

pub async fn count(db: &Db) -> SResult<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(db).await?)
}

pub async fn get(db: &Db, id: i64) -> SResult<Option<User>> {
    Ok(sqlx::query_as(concat!(
        "SELECT id, username, password_hash, session_version, last_login_at FROM users",
        " WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?)
}

pub async fn by_username(db: &Db, username: &str) -> SResult<Option<User>> {
    Ok(sqlx::query_as(concat!(
        "SELECT id, username, password_hash, session_version, last_login_at FROM users",
        " WHERE username = ?"
    ))
    .bind(username)
    .fetch_optional(db)
    .await?)
}

pub async fn create(db: &Db, username: &str, password: &str) -> SResult<User> {
    if let Some(e) = validate_username(username) {
        return Err(ServiceError::Validation(e));
    }
    let hash = hash_password(password).map_err(ServiceError::Internal)?;
    let now = now_ms();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, session_version, created_at, updated_at, password_changed_at) \
         VALUES (?, ?, 1, ?, ?, ?) RETURNING id",
    )
    .bind(username)
    .bind(hash)
    .bind(now)
    .bind(now)
    .bind(now)
    .fetch_one(db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            ServiceError::Conflict(format!("An account named '{username}' already exists."))
        }
        other => other.into(),
    })?;
    tracing::info!(username, "account created");
    get(db, id).await?.ok_or_else(|| ServiceError::Internal("account vanished".into()))
}

pub async fn authenticate(db: &Db, username: &str, password: &str) -> SResult<Option<User>> {
    let user = by_username(db, username).await?;
    let ok = verify_password(user.as_ref().map(|u| u.password_hash.as_str()), password);
    let Some(user) = user.filter(|_| ok) else { return Ok(None) };
    sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?").bind(now_ms()).bind(user.id).execute(db).await?;
    Ok(Some(user))
}

pub async fn set_password(db: &Db, username: &str, password: &str) -> SResult<User> {
    let Some(user) = by_username(db, username).await? else {
        return Err(ServiceError::NotFound(format!("No account named '{username}'.")));
    };
    let hash = hash_password(password).map_err(ServiceError::Internal)?;
    let now = now_ms();
    sqlx::query(
        "UPDATE users SET password_hash = ?, password_changed_at = ?, updated_at = ?, \
         session_version = session_version + 1 WHERE id = ?",
    )
    .bind(hash)
    .bind(now)
    .bind(now)
    .bind(user.id)
    .execute(db)
    .await?;
    tracing::info!(username, "password changed; other sessions signed out");
    get(db, user.id).await?.ok_or_else(|| ServiceError::Internal("account vanished".into()))
}

pub async fn list(db: &Db) -> SResult<Vec<User>> {
    Ok(sqlx::query_as(SELECT).fetch_all(db).await?)
}

fn prompt_line(label: &str) -> Result<String> {
    eprint!("{label}");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line)?;
    if read == 0 {
        bail!("input closed");
    }
    Ok(line.trim().to_string())
}

pub fn ask_new_account(default_username: Option<&str>) -> Result<(String, String)> {
    for _ in 0..3 {
        let label = match default_username {
            Some(d) => format!("Username [{d}]: "),
            None => "Username: ".into(),
        };
        let mut username = prompt_line(&label)?;
        if username.is_empty() {
            username = default_username.unwrap_or_default().to_string();
        }
        if username.is_empty() {
            bail!("no username entered");
        }
        if let Some(e) = validate_username(&username) {
            eprintln!("{e}");
            continue;
        }
        let password = rpassword::prompt_password("Password (min 12 characters): ")?;
        if password.is_empty() {
            bail!("no password entered");
        }
        let confirm = rpassword::prompt_password("Repeat password: ")?;
        if let Some(e) = validate_new_password(&password, &confirm, &username) {
            eprintln!("{e}");
            continue;
        }
        return Ok((username, password));
    }
    bail!("too many invalid attempts")
}

pub fn ask_new_password(username: &str) -> Result<String> {
    for _ in 0..3 {
        let password = rpassword::prompt_password("New password (min 12 characters): ")?;
        if password.is_empty() {
            bail!("no password entered");
        }
        let confirm = rpassword::prompt_password("Repeat password: ")?;
        match validate_new_password(&password, &confirm, username) {
            Some(e) => eprintln!("{e}"),
            None => return Ok(password),
        }
    }
    bail!("too many invalid attempts")
}

pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub async fn ensure_account(db: &Db, config: &Config) -> Result<()> {
    if count(db).await? > 0 {
        if config.admin_user.is_some() || config.admin_password.is_some() {
            tracing::info!("MPRIMCP_ADMIN_USER / MPRIMCP_ADMIN_PASSWORD ignored: an account already exists");
        }
        return Ok(());
    }
    let (username, password) = match (&config.admin_user, &config.admin_password) {
        (Some(u), Some(p)) => {
            if let Some(e) = validate_username(u).or_else(|| validate_new_password(p, p, u)) {
                bail!("MPRIMCP_ADMIN_USER / MPRIMCP_ADMIN_PASSWORD rejected: {e}");
            }
            (u.clone(), p.clone())
        }
        _ if interactive() => {
            eprintln!(
                "No account exists yet. Create the account that signs in to the dashboard and approves AI connectors."
            );
            ask_new_account(config.admin_user.as_deref())
                .map_err(|e| anyhow!("account not created ({e}); the server was not started"))?
        }
        _ => bail!(
            "No account exists yet and there is no terminal to create one.\n\
             Set MPRIMCP_ADMIN_USER and MPRIMCP_ADMIN_PASSWORD (min 12 characters) for the first start,\n\
             or run `mprimcp user create` once, then start the server again."
        ),
    };
    create(db, &username, &password).await.map_err(|e| anyhow!(e.to_string()))?;
    eprintln!("Account '{username}' created.");
    Ok(())
}
