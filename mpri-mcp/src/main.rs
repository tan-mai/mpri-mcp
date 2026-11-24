// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use mprimcp::app::App;
use mprimcp::auth::{passwords, users};
use mprimcp::config::{Config, Mode};
use mprimcp::{db, import, mcp, scheduler, web};

#[derive(Parser)]
#[command(name = "mprimcp", version, about = "Self-hosted MCP server that tracks supermarket prices (VND or USD)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// MCP over stdio for an AI client on this machine (no HTTP port, no cron)
    Stdio,
    /// MCP over Streamable HTTP at /mcp, the dashboard, and cron if MPRIMCP_CRON_ENABLED=true
    Serve {
        /// Listen address (default MPRIMCP_BIND or 127.0.0.1:8765)
        #[arg(long)]
        bind: Option<String>,
    },
    /// Only the dashboard (no MCP, no cron)
    Dashboard {
        /// Listen address (default MPRIMCP_BIND or 127.0.0.1:8765)
        #[arg(long)]
        bind: Option<String>,
    },
    /// Import data from the SQLite file of the Python release
    Import {
        /// Path to the Python release's mpri.db
        #[arg(long = "from")]
        from: PathBuf,
        /// Folder with telegram_bot_token / discord_webhook_url (default: secrets/ next to the file)
        #[arg(long)]
        secrets_dir: Option<PathBuf>,
    },
    /// Manage the dashboard account
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    /// Print the version
    Version,
}

#[derive(Subcommand)]
enum UserCommand {
    /// Create an account (asks for username and password)
    Create {
        #[arg(long)]
        username: Option<String>,
        /// Read the password from stdin (one line) instead of asking
        #[arg(long)]
        password_stdin: bool,
    },
    /// Change or reset a password; signs out every session of that account
    SetPassword {
        username: String,
        /// Read the password from stdin (one line) instead of asking
        #[arg(long)]
        password_stdin: bool,
    },
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
const QUIET_TARGETS: [&str; 7] = ["rmcp", "hyper", "hyper_util", "h2", "reqwest", "rustls", "sqlx"];

fn init_logging() {
    use std::io::IsTerminal;
    use tracing_subscriber::filter::filter_fn;
    use tracing_subscriber::prelude::*;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,rmcp=warn,sqlx=warn"));
    let no_payloads = filter_fn(|meta| {
        *meta.level() <= tracing::Level::INFO
            || !QUIET_TARGETS.iter().any(|t| meta.target() == *t || meta.target().starts_with(&format!("{t}::")))
    });
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(false)
        .with_filter(filter)
        .with_filter(no_payloads);
    tracing_subscriber::registry().with(fmt).init();
}

fn read_stdin_password() -> Result<String> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

async fn shutdown_signal(cancel: CancellationToken) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    cancel.cancel();
}

async fn run_http(mode: Mode, bind: Option<String>) -> Result<()> {
    let config = Config::from_env(bind.as_deref())?;
    let app = App::open(config, mode).await?;
    if let Some(from) = app.config.import_from.clone() {
        import::auto_import(&app.db, &from).await?;
    }
    users::ensure_account(&app.db, &app.config).await?;
    let state = web::AppState::new(app.clone()).await?;
    let cancel = CancellationToken::new();
    tokio::spawn(shutdown_signal(cancel.clone()));
    let mcp_enabled = mode == Mode::Serve;
    if mcp_enabled {
        tokio::spawn(scheduler::run(app.clone(), cancel.clone()));
    }
    let url = format!("{}/admin", app.config.public_url);
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        mode = mode.as_str(),
        bind = %app.config.bind,
        data_dir = %app.config.data_dir.display(),
        cron = app.server_cron(),
        "mprimcp started"
    );
    eprintln!("Dashboard: {url}");
    if mcp_enabled {
        eprintln!("MCP endpoint: {}", app.config.mcp_url());
    }
    web::serve(state, mcp_enabled, cancel).await?;
    app.notifier.shutdown(std::time::Duration::from_secs(5)).await;
    Ok(())
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Version => {
            println!("mprimcp {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Stdio => {
            let config = Config::from_env(None)?;
            let app = App::open(config, Mode::Stdio).await?;
            mcp::run_stdio(app).await
        }
        Command::Serve { bind } => run_http(Mode::Serve, bind).await,
        Command::Dashboard { bind } => run_http(Mode::Dashboard, bind).await,
        Command::Import { from, secrets_dir } => {
            let config = Config::from_env(None)?;
            let target = db::open(&config.db_path()).await?;
            let report = import::import(&target, &from, secrets_dir.as_deref()).await?;
            println!("Imported {} into {}", from.display(), config.db_path().display());
            println!("{:<24} {:>8} {:>9} {:>8}", "table", "source", "inserted", "skipped");
            for t in &report.tables {
                println!("{:<24} {:>8} {:>9} {:>8}", t.table, t.source, t.inserted, t.skipped());
            }
            for n in &report.notes {
                println!("- {n}");
            }
            Ok(())
        }
        Command::User { command } => {
            let config = Config::from_env(None)?;
            let pool = db::open(&config.db_path()).await?;
            match command {
                UserCommand::Create { username, password_stdin } => {
                    let (username, password) = if password_stdin {
                        let username =
                            username.ok_or_else(|| anyhow!("--username is required with --password-stdin"))?;
                        let password = read_stdin_password()?;
                        if let Some(e) = passwords::validate_new_password(&password, &password, &username) {
                            return Err(anyhow!(e));
                        }
                        (username, password)
                    } else {
                        users::ask_new_account(username.as_deref())?
                    };
                    let user = users::create(&pool, &username, &password).await.map_err(|e| anyhow!(e.to_string()))?;
                    println!("Account '{}' created.", user.username);
                }
                UserCommand::SetPassword { username, password_stdin } => {
                    if users::by_username(&pool, &username).await?.is_none() {
                        return Err(anyhow!("No account named '{username}'. Use `mprimcp user create`."));
                    }
                    let password = if password_stdin {
                        let p = read_stdin_password()?;
                        if let Some(e) = passwords::validate_new_password(&p, &p, &username) {
                            return Err(anyhow!(e));
                        }
                        p
                    } else {
                        users::ask_new_password(&username)?
                    };
                    users::set_password(&pool, &username, &password).await.map_err(|e| anyhow!(e.to_string()))?;
                    println!("Password set for '{username}'. All sessions were signed out.");
                }
            }
            Ok(())
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    mprimcp::app::install_crypto_provider();
    let cli = Cli::parse();
    init_logging();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
