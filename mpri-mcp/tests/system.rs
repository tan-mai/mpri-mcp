// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
mod common;

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;

use common::*;
use mprimcp::app::App;
use mprimcp::auth::{oauth, users};
use mprimcp::config::Mode;
use mprimcp::{db, import};
use serde_json::{Value, json};
use sqlx::sqlite::SqliteConnectOptions;

#[test]
fn ac24_agents_md_matches_server_instructions() {
    let agents = std::fs::read_to_string("../AGENTS.md").unwrap().replace("\r\n", "\n");
    let instructions = mprimcp::mcp::instructions();
    assert!(agents.contains(&instructions), "AGENTS.md is out of date: regenerate it from assets/instructions.md");
    assert!(agents.contains(mprimcp::mcp::INIT_PROMPT.replace("\r\n", "\n").trim_end()));
    for block in ["WORKFLOW 0 - ONBOARDING", "WORKFLOW A - AGENT CRAWL", "WORKFLOW B - ADD A PRODUCT BY SEARCH"] {
        assert!(instructions.contains(block), "{block}");
    }
    assert!(!instructions.contains("Claude Browser"));
}

#[tokio::test]
async fn ac16_ac17_ac17b_first_account() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), false, "http://127.0.0.1:9");
    let app = App::open(cfg.clone(), Mode::Serve).await.unwrap();
    let err = users::ensure_account(&app.db, &cfg).await.unwrap_err().to_string();
    assert!(err.contains("MPRIMCP_ADMIN_USER") && err.contains("mprimcp user create"), "{err}");
    assert_eq!(users::count(&app.db).await.unwrap(), 0);

    cfg.admin_user = Some("tan".into());
    cfg.admin_password = Some("short".into());
    assert!(users::ensure_account(&app.db, &cfg).await.is_err());

    cfg.admin_password = Some("first-password-123".into());
    users::ensure_account(&app.db, &cfg).await.unwrap();
    assert!(users::authenticate(&app.db, "tan", "first-password-123").await.unwrap().is_some());

    cfg.admin_user = Some("other".into());
    cfg.admin_password = Some("second-password-456".into());
    users::ensure_account(&app.db, &cfg).await.unwrap();
    assert_eq!(users::count(&app.db).await.unwrap(), 1);
    assert!(users::authenticate(&app.db, "tan", "first-password-123").await.unwrap().is_some());
    assert!(users::authenticate(&app.db, "other", "second-password-456").await.unwrap().is_none());
}

struct StdioProc {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl StdioProc {
    fn spawn(data_dir: &Path) -> Self {
        Self::spawn_with(data_dir, &[], Stdio::null())
    }

    fn spawn_with(data_dir: &Path, envs: &[(&str, &str)], stderr: Stdio) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mprimcp"));
        cmd.arg("stdio")
            .env("MPRIMCP_DATA_DIR", data_dir)
            .env("RUST_LOG", "debug")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        let mut p = Self { child, stdin, lines: rx };
        p.send(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}}));
        p.recv(0);
        p.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        p
    }

    fn send(&mut self, msg: Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn recv(&self, id: i64) -> Value {
        loop {
            let line = self.lines.recv_timeout(std::time::Duration::from_secs(30)).expect("no response");
            let msg: Value = serde_json::from_str(&line).unwrap_or_else(|_| panic!("stdout is not JSON-RPC: {line}"));
            assert_eq!(msg["jsonrpc"], "2.0", "{line}");
            if msg["id"] == id {
                return msg;
            }
        }
    }

    fn call_any(&mut self, id: i64, name: &str, args: Value) -> Value {
        self.send(
            json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}}),
        );
        self.recv(id)
    }

    fn call(&mut self, id: i64, name: &str, args: Value) -> Value {
        self.send(
            json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}}),
        );
        let msg = self.recv(id);
        assert_eq!(msg["result"]["isError"], false, "{msg}");
        msg["result"]["structuredContent"].clone()
    }
}

impl Drop for StdioProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[test]
fn ac02_ac17f_stdio_binary_on_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("fresh");
    let start = std::time::Instant::now();
    let mut p = StdioProc::spawn(&data);
    let startup = start.elapsed();
    let out = p.call(1, "get_supermarkets", json!({}));
    assert_eq!(out["supermarkets"], json!([]));
    assert!(data.join("mprimcp.db").exists());
    assert!(startup.as_secs_f64() < 5.0, "startup took {startup:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ac18_two_processes_claim_without_duplicates() {
    let h = harness().await;
    for (name, base) in [("Bách Hóa Xanh", "bachhoaxanh.com"), ("WinMart", "winmart.vn")] {
        let id = h.seed_market(name, base, 0).await;
        for i in 0..20 {
            h.call(
                "add_product_with_urls",
                json!({"name": format!("{name} hàng {i}"), "urls": [{"supermarket_id": id, "url": format!("https://{base}/p/{i}")}]}),
            )
            .await;
        }
        h.call("trigger_crawl_now", json!({"supermarket_id": id})).await;
    }
    let data = h.dir.path().to_path_buf();
    let worker = std::thread::spawn(move || {
        let mut p = StdioProc::spawn(&data);
        let mut ids = Vec::new();
        for n in 0..10 {
            let out = p.call(100 + n, "get_pending_crawl_tasks", json!({"limit": 3}));
            for t in out["tasks"].as_array().unwrap() {
                ids.push(t["task_id"].as_str().unwrap().to_string());
            }
        }
        let results: Vec<Value> = ids.iter().map(|id| json!({"task_id": id, "status": "ok", "price": 10000})).collect();
        for chunk in results.chunks(20) {
            p.call(500, "submit_price_results", json!({"results": chunk}));
        }
        ids
    });
    let other = App::open(config(h.dir.path(), false, "http://127.0.0.1:9"), Mode::Dashboard).await.unwrap();
    let mut local = Vec::new();
    for _ in 0..10 {
        let (a, b) = tokio::join!(
            mprimcp::services::agent_tasks::claim_tasks(&h.app.db, None, 1),
            mprimcp::services::agent_tasks::claim_tasks(&other.db, None, 1),
        );
        for r in [a.unwrap(), b.unwrap()] {
            local.extend(r.tasks.into_iter().map(|t| t.task.id));
        }
    }
    let remote = tokio::task::spawn_blocking(move || worker.join().unwrap()).await.unwrap();
    let total = local.len() + remote.len();
    let unique: HashSet<&String> = local.iter().chain(remote.iter()).collect();
    assert_eq!(unique.len(), total, "a task was claimed twice");
    assert_eq!(total, 40);
    assert_eq!(h.sql_i64("SELECT COUNT(*) FROM crawl_tasks WHERE status = 'done'").await, remote.len() as i64);
}

async fn legacy_db(path: &Path) {
    let pool = sqlx::SqlitePool::connect_with(
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal),
    )
    .await
    .unwrap();
    let schema = include_str!("fixtures/legacy_schema.sql");
    sqlx::raw_sql(sqlx::AssertSqlSafe(schema.to_string())).execute(&pool).await.unwrap();
    let data = r#"
INSERT INTO supermarkets VALUES ('winmart','WinMart','https://winmart.vn','0 8 * * 6,0',1,'Asia/Ho_Chi_Minh','https://winmart.vn/s?q={q}','2026-10-01 01:00:00.000000','2026-10-02 01:00:00.000000');
INSERT INTO supermarkets VALUES ('bach-hoa-xanh','Bách Hóa Xanh','https://bachhoaxanh.com',NULL,0,'Asia/Ho_Chi_Minh',NULL,'2026-10-01 01:00:00.000000','2026-10-01 01:00:00.000000');
INSERT INTO products VALUES (1,'Sữa TH True Milk Ít Đường 110ml Thùng 48 Hộp','sua th true milk it duong 110ml thung 48 hop','Sữa','2026-10-01 02:00:00.000000');
INSERT INTO product_urls VALUES (1,1,'winmart','https://winmart.vn/p/th',1,'2026-11-20 01:00:00.000000','2026-11-20 01:00:00.000000',NULL,0,1,'2026-10-01 02:00:00.000000');
INSERT INTO product_urls VALUES (2,1,'bach-hoa-xanh','https://bachhoaxanh.com/p/th',1,NULL,NULL,NULL,0,NULL,'2026-10-01 02:00:00.000000');
INSERT INTO price_records VALUES (1,1,420000,450000,NULL,1,'2026-11-19 01:00:00.000000','agent',0,'420.000đ');
INSERT INTO price_records VALUES (2,1,399000,450000,389000,1,'2026-11-20 01:00:00.123456','agent',0,'399.000đ');
INSERT INTO price_records VALUES (3,2,405000,405000,NULL,1,'2026-11-20 02:00:00.000000','agent',0,NULL);
INSERT INTO crawl_runs VALUES (1,'winmart','agent','success','2026-11-20 00:59:00.000000','2026-11-20 01:00:00.000000',1,1,0,NULL);
INSERT INTO crawl_tasks VALUES ('abc123def456',1,1,'winmart','https://winmart.vn/p/th','done',1,'2026-11-20 00:59:00.000000','2026-11-20 00:59:30.000000',NULL,'2026-11-20 01:00:00.000000',1,NULL);
INSERT INTO price_alerts VALUES (1,1,400000,'["winmart"]','{"winmart": 399000}','2026-11-01 00:00:00.000000','2026-11-01 00:00:00.000000');
INSERT INTO app_settings VALUES ('notifications','{"telegram_enabled": true, "telegram_chat_id": "42", "discord_enabled": false, "summary_enabled": true, "last_summary_at": "2026-11-20T01:00:00+00:00"}','2026-11-20 01:00:00.000000');
INSERT INTO admin_users VALUES (1,'tan','$argon2id$v=19$m=65536,t=3,p=4$N90Mo7aOkXmX/vM+y3vEkg$BbNTVKqBvPMfgshqI8YxsgnSnbpaE2V0/o/1NL+KZNw',3,'2026-10-01 00:00:00.000000','2026-11-20 00:00:00.000000','2026-10-01 00:00:00.000000');
INSERT INTO oauth_clients VALUES ('client-1',NULL,'Claude','{"redirect_uris": ["https://claude.ai/api/mcp/auth_callback"], "token_endpoint_auth_method": "none", "grant_types": ["authorization_code", "refresh_token"], "response_types": ["code"], "scope": "mpri", "client_name": "Claude", "client_uri": null, "client_id": "client-1", "client_id_issued_at": 1760000000, "client_secret_expires_at": null}','2026-10-01 00:00:00.000000',NULL,NULL);
"#;
    sqlx::raw_sql(data).execute(&pool).await.unwrap();
    let far = "2099-01-01 00:00:00.000000";
    for (token, kind) in [("mpri_at_legacy", "access"), ("mpri_rt_legacy", "refresh")] {
        sqlx::query("INSERT INTO oauth_tokens VALUES (?, ?, 'client-1', '[\"mpri\"]', 'http://localhost:8765/mcp', 'tan', 'fam1', '2026-11-20 00:00:00.000000', ?, NULL, NULL, NULL)")
            .bind(oauth::token_hash(token))
            .bind(kind)
            .bind(far)
            .execute(&pool)
            .await
            .unwrap();
    }
    pool.close().await;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(path, perms).unwrap();
}

#[tokio::test]
async fn ac22_import_python_database_twice() {
    let src_dir = tempfile::tempdir().unwrap();
    let source = src_dir.path().join("mpri.db");
    legacy_db(&source).await;
    std::fs::create_dir_all(src_dir.path().join("secrets")).unwrap();
    std::fs::write(
        src_dir.path().join("secrets/telegram_bot_token"),
        "123456789:AAbbccddeeffgghhiijjkkllmmnnoopp_xyz\n",
    )
    .unwrap();

    let h = harness().await;
    let first = import::import(&h.app.db, &source, None).await.unwrap();
    let count = |r: &import::ImportReport, t: &str| {
        r.tables.iter().find(|s| s.table == t).map(|s| (s.source, s.inserted)).unwrap()
    };
    assert_eq!(count(&first, "supermarkets"), (2, 2));
    assert_eq!(count(&first, "price_records"), (3, 3));
    assert_eq!(count(&first, "price_alerts"), (1, 1));
    assert_eq!(count(&first, "users"), (1, 1));
    assert_eq!(count(&first, "oauth_tokens"), (2, 2));
    assert_eq!(count(&first, "notification_channels"), (1, 1));
    let second = import::import(&h.app.db, &source, None).await.unwrap();
    assert!(second.tables.iter().all(|t| t.inserted == 0), "{:?}", second.tables);

    let cmp = h.call("get_product_price_comparison", json!({"product_name": "sua th it duong"})).await;
    assert_eq!(cmp["prices"][0]["supermarket_id"], "winmart");
    assert_eq!(cmp["prices"][0]["current_price"], 399000);
    assert_eq!(cmp["prices"][0]["original_price"], 450000);
    assert_eq!(cmp["prices"][0]["discount_pct"], 11.3);
    assert_eq!(cmp["prices"][0]["member_discount_pct"], 13.6);
    assert_eq!(cmp["prices"][1]["supermarket_id"], "bach-hoa-xanh");
    assert_eq!(cmp["cheapest"]["current_price"], 399000);
    let alerts = h.call("get_price_alerts", json!({})).await;
    assert_eq!(alerts["alerts"][0]["supermarket_ids"], json!(["winmart"]));
    assert_eq!(alerts["alerts"][0]["below_target_now"], true);
    let markets = h.call("get_supermarkets", json!({})).await;
    assert_eq!(markets["supermarkets"][1]["schedule"]["cron_expression"], "0 8 * * 6,0");
    assert_eq!(markets["supermarkets"][1]["last_run"]["status"], "success");
    assert!(users::authenticate(&h.app.db, "tan", "python-password-123").await.unwrap().is_some());
    let token = oauth::load_access_token(&h.app.db, "mpri_at_legacy").await.unwrap().unwrap();
    assert_eq!(token.client_id, "client-1");
    assert_eq!(token.scopes, vec!["mpri".to_string()]);
    let status = h.call("get_setup_status", json!({})).await;
    assert_eq!(status["notification_channels"][0]["target_masked"], "123456***xyz");
    let client = oauth::get_client(&h.app.db, "client-1").await.unwrap().unwrap();
    assert_eq!(client.redirect_uris, vec!["https://claude.ai/api/mcp/auth_callback".to_string()]);

    let fresh = harness().await;
    let auto = import::auto_import(&fresh.app.db, &source).await.unwrap().unwrap();
    assert_eq!(count(&auto, "supermarkets"), (2, 2));
    assert!(import::auto_import(&fresh.app.db, &source).await.unwrap().is_none());

    let again = db::open(&h.dir.path().join("mprimcp.db")).await.unwrap();
    let err = import::import(&again, &h.dir.path().join("mprimcp.db"), None).await.unwrap_err().to_string();
    assert!(err.contains("already a Rust MPriMCP database"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac13_token_never_in_logs_or_outputs() {
    let token = "987654321:ZZyyxxwwvvuuttssrrqqppoonnmmllkk_abc";
    let (ok_api, _ok) = spawn_mock_telegram(true).await;
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("stderr.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let data = dir.path().join("data");
    let api = ok_api.clone();
    let outputs = tokio::task::spawn_blocking(move || {
        let mut p = StdioProc::spawn_with(&data, &[("MPRIMCP_TELEGRAM_API", &api)], Stdio::from(log));
        let mut outs = Vec::new();
        outs.push(p.call_any(
            1,
            "set_notification_channel",
            json!({"channel": "telegram", "token": token, "chat_id": "42"}),
        ));
        outs.push(p.call_any(3, "get_setup_status", json!({})));
        outs.push(p.call_any(4, "set_notification_channel", json!({"channel": "telegram", "token": "bad"})));
        drop(p);
        outs
    })
    .await
    .unwrap();
    assert_eq!(outputs[0]["result"]["structuredContent"]["test_ok"], true, "{}", outputs[0]);
    let all = serde_json::to_string(&outputs).unwrap();
    let logs = std::fs::read_to_string(&log_path).unwrap();
    for secret in [token, "ZZyyxxwwvvuuttssrrqqppoonnmmllkk"] {
        assert!(!all.contains(secret), "tool output leaks {secret}");
        assert!(!logs.contains(secret), "log leaks {secret}");
    }
    assert!(logs.contains("notification channel saved"), "{logs}");
}

#[tokio::test]
async fn ac22_import_reads_uncheckpointed_wal() {
    let src_dir = tempfile::tempdir().unwrap();
    let source = src_dir.path().join("mpri.db");
    legacy_db(&source).await;
    let mut perms = std::fs::metadata(&source).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&source, perms).unwrap();
    let writer = sqlx::SqlitePool::connect_with(
        SqliteConnectOptions::new().filename(&source).journal_mode(sqlx::sqlite::SqliteJournalMode::Wal),
    )
    .await
    .unwrap();
    sqlx::query("PRAGMA wal_autocheckpoint = 0").execute(&writer).await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO supermarkets VALUES ('lotte','LOTTE Mart','https://lottemart.vn',NULL,0,'Asia/Ho_Chi_Minh',NULL,\
         '2026-11-21 01:00:00.000000','2026-11-21 01:00:00.000000');",
    )
    .execute(&writer)
    .await
    .unwrap();
    assert!(std::fs::metadata(src_dir.path().join("mpri.db-wal")).unwrap().len() > 0);
    let h = harness().await;
    let report = import::import(&h.app.db, &source, None).await.unwrap();
    let markets = report.tables.iter().find(|t| t.table == "supermarkets").unwrap();
    assert_eq!((markets.source, markets.inserted), (3, 3));
    writer.close().await;
}
