// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::Path;

use mprimcp::app::App;
use mprimcp::config::{Config, Mode};
use mprimcp::mcp::MprServer;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tempfile::TempDir;

pub fn config(dir: &Path, cron: bool, telegram_api: &str) -> Config {
    Config {
        data_dir: dir.to_path_buf(),
        bind: "127.0.0.1:0".parse().unwrap(),
        public_url: "http://localhost:8765".into(),
        cron_enabled: cron,
        admin_user: None,
        admin_password: None,
        trust_proxy: false,
        extra_redirect_uris: Vec::new(),
        telegram_api: telegram_api.into(),
        import_from: None,
        currency: None,
        language: None,
        timezone: None,
    }
}

pub struct Harness {
    pub app: App,
    pub client: RunningService<RoleClient, ()>,
    pub dir: TempDir,
}

pub async fn harness() -> Harness {
    harness_with(Mode::Stdio, false, "http://127.0.0.1:9").await
}

pub async fn harness_with(mode: Mode, cron: bool, telegram_api: &str) -> Harness {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    let app = App::open(config(dir.path(), cron, telegram_api), mode).await.unwrap();
    let client = connect(app.clone()).await;
    Harness { app, client, dir }
}

pub async fn connect(app: App) -> RunningService<RoleClient, ()> {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let running = MprServer::new(app).serve(server_io).await.unwrap();
        let _ = running.waiting().await;
    });
    ().serve(client_io).await.unwrap()
}

pub async fn call_raw(client: &RunningService<RoleClient, ()>, name: &str, args: Value) -> (bool, Value, String) {
    let params =
        CallToolRequestParams::new(name.to_string()).with_arguments(args.as_object().cloned().unwrap_or_default());
    let result = client.call_tool(params).await.unwrap_or_else(|e| panic!("{name}: {e}"));
    let text = result.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect::<Vec<_>>().join("\n");
    (result.is_error.unwrap_or(false), result.structured_content.unwrap_or(Value::Null), text)
}

impl Harness {
    pub async fn call(&self, name: &str, args: Value) -> Value {
        let (err, value, text) = call_raw(&self.client, name, args).await;
        assert!(!err, "{name} failed: {text}");
        value
    }

    pub async fn call_err(&self, name: &str, args: Value) -> String {
        let (err, _, text) = call_raw(&self.client, name, args).await;
        assert!(err, "{name} should fail, got {text}");
        text
    }

    pub async fn sql_i64(&self, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.app.db).await.unwrap()
    }

    pub async fn seed_market(&self, name: &str, base: &str, urls: usize) -> String {
        let m = self.call("add_supermarket", json!({"name": name, "base_url": base})).await;
        let id = m["supermarket_id"].as_str().unwrap().to_string();
        for i in 0..urls {
            self.call(
                "add_product_with_urls",
                json!({
                    "name": format!("Sản phẩm {i}"),
                    "urls": [{"supermarket_id": id, "url": format!("https://{base}/p/{i}")}],
                }),
            )
            .await;
        }
        id
    }
}

pub async fn spawn_mock_telegram(ok: bool) -> (String, tokio::task::JoinHandle<()>) {
    use axum::Router;
    use axum::routing::post;
    let app = Router::new().route(
        "/{bot}/sendMessage",
        post(move || async move {
            if ok {
                (axum::http::StatusCode::OK, axum::Json(json!({"ok": true, "result": {}})))
            } else {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    axum::Json(json!({"ok": false, "error_code": 401, "description": "Unauthorized"})),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), handle)
}
