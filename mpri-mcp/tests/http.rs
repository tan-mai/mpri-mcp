// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http_body_util::BodyExt;
use mprimcp::app::App;
use mprimcp::auth::users;
use mprimcp::config::Mode;
use mprimcp::locale::Lang;
use mprimcp::web::{self, AppState};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const PASSWORD: &str = "correct-horse-battery";
const CALLBACK: &str = "https://claude.ai/api/mcp/auth_callback";

struct Server {
    router: Router,
    app: App,
    _dir: tempfile::TempDir,
}

async fn server(mode: Mode) -> Server {
    server_lang(mode, None).await
}

async fn server_lang(mode: Mode, language: Option<Lang>) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let mut config = common::config(dir.path(), false, "http://127.0.0.1:9");
    config.language = language;
    let app = App::open(config, mode).await.unwrap();
    users::create(&app.db, "tan", PASSWORD).await.unwrap();
    let state = AppState::new(app.clone()).await.unwrap();
    let router = web::router(state, mode == Mode::Serve, CancellationToken::new());
    Server { router, app, _dir: dir }
}

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: String,
}

impl Resp {
    fn location(&self) -> String {
        self.headers.get(header::LOCATION).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default()
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

async fn send(router: &Router, req: Request<Body>) -> Resp {
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).to_string();
    Resp { status, headers, body }
}

#[derive(Default)]
struct Browser {
    cookie: Option<String>,
}

impl Browser {
    async fn request(&mut self, router: &Router, method: &str, uri: &str, form: Option<&[(&str, &str)]>) -> Resp {
        let mut b = Request::builder().method(method).uri(uri).header(header::HOST, "localhost:8765");
        if let Some(c) = &self.cookie {
            b = b.header(header::COOKIE, c);
        }
        let body = match form {
            Some(f) => {
                b = b.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
                Body::from(serde_urlencoded::to_string(f).unwrap())
            }
            None => Body::empty(),
        };
        let resp = send(router, b.body(body).unwrap()).await;
        if let Some(set) = resp.headers.get(header::SET_COOKIE) {
            self.cookie = Some(set.to_str().unwrap().split(';').next().unwrap().to_string());
        }
        resp
    }

    async fn get(&mut self, router: &Router, uri: &str) -> Resp {
        self.request(router, "GET", uri, None).await
    }

    async fn post(&mut self, router: &Router, uri: &str, form: &[(&str, &str)]) -> Resp {
        self.request(router, "POST", uri, Some(form)).await
    }

    async fn csrf(&mut self, router: &Router, uri: &str) -> String {
        let page = self.get(router, uri).await;
        let marker = "name=\"csrf_token\" value=\"";
        let start = page.body.find(marker).unwrap_or_else(|| panic!("no csrf on {uri}: {}", page.body)) + marker.len();
        page.body[start..start + page.body[start..].find('"').unwrap()].to_string()
    }

    async fn login(&mut self, router: &Router, password: &str) -> Resp {
        let csrf = self.csrf(router, "/admin/login").await;
        self.post(
            router,
            "/admin/login",
            &[("csrf_token", &csrf), ("username", "tan"), ("password", password), ("next", "/admin")],
        )
        .await
    }
}

fn pkce() -> (String, String) {
    let verifier = "verifier-0123456789-abcdefghijklmnopqrstuvwxyz-ABCDEFG".to_string();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn query_param(url: &str, key: &str) -> Option<String> {
    url::Url::parse(url).ok()?.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.to_string())
}

async fn post_json(router: &Router, uri: &str, body: Value, bearer: Option<&str>) -> Resp {
    let mut b = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "localhost:8765")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18");
    if let Some(t) = bearer {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    send(router, b.body(Body::from(body.to_string())).unwrap()).await
}

async fn post_form(router: &Router, uri: &str, form: &[(&str, &str)]) -> Resp {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::HOST, "localhost:8765")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(serde_urlencoded::to_string(form).unwrap()))
        .unwrap();
    send(router, req).await
}

fn initialize() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "claude-ai", "version": "1"}}})
}

async fn connect_client(s: &Server, browser: &mut Browser) -> (String, Value) {
    let reg = post_json(
        &s.router,
        "/register",
        json!({"redirect_uris": [CALLBACK], "token_endpoint_auth_method": "none", "client_name": "Claude",
               "grant_types": ["authorization_code", "refresh_token"], "response_types": ["code"]}),
        None,
    )
    .await;
    assert_eq!(reg.status, StatusCode::CREATED, "{}", reg.body);
    let client_id = reg.json()["client_id"].as_str().unwrap().to_string();
    let (verifier, challenge) = pkce();
    let authorize = format!(
        "/authorize?response_type=code&client_id={client_id}&redirect_uri={}&code_challenge={challenge}\
         &code_challenge_method=S256&state=st4te&scope=mpri&resource={}",
        urlencode(CALLBACK),
        urlencode("http://localhost:8765/mcp")
    );
    let r = browser.get(&s.router, &authorize).await;
    assert_eq!(r.status, StatusCode::FOUND, "{}", r.body);
    let consent = r.location();
    assert!(consent.starts_with("/oauth/consent?req="), "{consent}");
    let r = browser.get(&s.router, &consent).await;
    if r.status == StatusCode::SEE_OTHER {
        assert!(r.location().starts_with("/admin/login?next="));
        let r = browser.login(&s.router, PASSWORD).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
    }
    let csrf = browser.csrf(&s.router, &consent).await;
    let req = query_param(&format!("http://x{consent}"), "req").unwrap();
    let r =
        browser.post(&s.router, "/oauth/consent", &[("csrf_token", &csrf), ("req", &req), ("decision", "allow")]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let back = r.location();
    assert!(back.starts_with(CALLBACK), "{back}");
    assert_eq!(query_param(&back, "state").as_deref(), Some("st4te"));
    assert_eq!(query_param(&back, "iss").as_deref(), Some("http://localhost:8765"));
    let code = query_param(&back, "code").unwrap();
    let tok = post_form(
        &s.router,
        "/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", CALLBACK),
            ("client_id", &client_id),
            ("code_verifier", &verifier),
        ],
    )
    .await;
    assert_eq!(tok.status, StatusCode::OK, "{}", tok.body);
    let mut out = tok.json();
    out["code"] = json!(code);
    out["verifier"] = json!(verifier);
    (client_id, out)
}

fn urlencode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

#[tokio::test]
async fn ac17c_ac23_oauth_flow_and_bearer_protected_mcp() {
    let s = server(Mode::Serve).await;
    let meta =
        send(&s.router, Request::get("/.well-known/oauth-authorization-server").body(Body::empty()).unwrap()).await;
    assert_eq!(meta.json()["issuer"], "http://localhost:8765");
    assert_eq!(meta.json()["token_endpoint"], "http://localhost:8765/token");
    assert_eq!(meta.headers.get("access-control-allow-origin").unwrap(), "*");
    let pr =
        send(&s.router, Request::get("/.well-known/oauth-protected-resource/mcp").body(Body::empty()).unwrap()).await;
    assert_eq!(pr.json()["resource"], "http://localhost:8765/mcp");

    let anon = post_json(&s.router, "/mcp", initialize(), None).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
    let www = anon.headers.get(header::WWW_AUTHENTICATE).unwrap().to_str().unwrap();
    assert!(www.contains("resource_metadata=\"http://localhost:8765/.well-known/oauth-protected-resource/mcp\""));
    let bad = post_json(&s.router, "/mcp", initialize(), Some("mpri_at_nope")).await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);

    let evil = post_json(&s.router, "/register", json!({"redirect_uris": ["https://evil.example/cb"]}), None).await;
    assert_eq!(evil.json()["error"], "invalid_redirect_uri");

    let mut browser = Browser::default();
    let (client_id, tokens) = connect_client(&s, &mut browser).await;
    let access = tokens["access_token"].as_str().unwrap().to_string();
    let refresh = tokens["refresh_token"].as_str().unwrap().to_string();
    assert!(access.starts_with("mpri_at_") && refresh.starts_with("mpri_rt_"));
    assert_eq!(tokens["token_type"], "Bearer");
    assert_eq!(tokens["expires_in"], 3600);

    let init = post_json(&s.router, "/mcp", initialize(), Some(&access)).await;
    assert_eq!(init.status, StatusCode::OK, "{}", init.body);
    assert!(init.body.contains("\"name\":\"mprimcp\""), "{}", init.body);
    let call = post_json(
        &s.router,
        "/mcp",
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "get_supermarkets", "arguments": {}}}),
        Some(&access),
    )
    .await;
    assert_eq!(call.status, StatusCode::OK, "{}", call.body);
    assert!(call.body.contains("Asia/Ho_Chi_Minh"), "{}", call.body);

    let rotated = post_form(
        &s.router,
        "/token",
        &[("grant_type", "refresh_token"), ("refresh_token", &refresh), ("client_id", &client_id)],
    )
    .await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.body);
    let new_access = rotated.json()["access_token"].as_str().unwrap().to_string();
    assert_eq!(post_json(&s.router, "/mcp", initialize(), Some(&new_access)).await.status, StatusCode::OK);
    let reuse = post_form(
        &s.router,
        "/token",
        &[("grant_type", "refresh_token"), ("refresh_token", &refresh), ("client_id", &client_id)],
    )
    .await;
    assert_eq!(reuse.json()["error"], "invalid_grant");
    assert_eq!(post_json(&s.router, "/mcp", initialize(), Some(&new_access)).await.status, StatusCode::UNAUTHORIZED);

    let (client2, second) = connect_client(&s, &mut browser).await;
    let access2 = second["access_token"].as_str().unwrap().to_string();
    assert_eq!(post_json(&s.router, "/mcp", initialize(), Some(&access2)).await.status, StatusCode::OK);
    let code = second["code"].as_str().unwrap();
    let verifier = second["verifier"].as_str().unwrap();
    let replay = post_form(
        &s.router,
        "/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", CALLBACK),
            ("client_id", &client2),
            ("code_verifier", verifier),
        ],
    )
    .await;
    assert_eq!(replay.json()["error"], "invalid_grant");
    assert_eq!(post_json(&s.router, "/mcp", initialize(), Some(&access2)).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ac17c_dashboard_requires_login_and_ac25_uses_service_logic() {
    let s = server_lang(Mode::Dashboard, Some(Lang::Vi)).await;
    let mut browser = Browser::default();
    let r = browser.get(&s.router, "/admin").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), "/admin/login?next=%2Fadmin");
    let mcp = post_json(&s.router, "/mcp", initialize(), None).await;
    assert_eq!(mcp.status, StatusCode::NOT_FOUND, "dashboard mode has no MCP endpoint");

    assert_eq!(browser.login(&s.router, PASSWORD).await.status, StatusCode::SEE_OTHER);
    let page = browser.get(&s.router, "/admin").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Thêm siêu thị"));
    assert!(page.headers.get("content-security-policy").is_some());
    let cookie = page.headers.get(header::SET_COOKIE).map(|c| c.to_str().unwrap().to_string());
    if let Some(c) = cookie {
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict"));
    }

    let csrf = browser.csrf(&s.router, "/admin/supermarkets").await;
    let r = browser
        .post(
            &s.router,
            "/admin/supermarkets",
            &[("csrf_token", &csrf), ("name", "Bách Hóa Xanh"), ("base_url", "bachhoaxanh.com")],
        )
        .await;
    assert_eq!(r.location(), "/admin/supermarkets/bach-hoa-xanh");
    let r = browser
        .post(
            &s.router,
            "/admin/supermarkets",
            &[("csrf_token", &csrf), ("name", "bach hoa xanh"), ("base_url", "x.vn")],
        )
        .await;
    assert_eq!(r.location(), "/admin/supermarkets");
    let list = browser.get(&s.router, "/admin/supermarkets").await;
    assert!(list.body.contains("already exists with ID"), "{}", list.body);
    let r = browser
        .post(&s.router, "/admin/notifications/telegram", &[("csrf_token", &csrf), ("token", "bad"), ("chat_id", "1")])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let page = browser.get(&s.router, "/admin/notifications").await;
    assert!(page.body.contains("Telegram bot token"), "{}", page.body);
}

#[tokio::test]
async fn ac27_form_without_csrf_rejected() {
    let s = server(Mode::Dashboard).await;
    let mut browser = Browser::default();
    browser.login(&s.router, PASSWORD).await;
    let r = browser.post(&s.router, "/admin/supermarkets", &[("name", "X Mart"), ("base_url", "x.vn")]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = browser
        .post(&s.router, "/admin/supermarkets", &[("csrf_token", "forged"), ("name", "X Mart"), ("base_url", "x.vn")])
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let mut stranger = Browser::default();
    let r = stranger.post(&s.router, "/admin/login", &[("username", "tan"), ("password", PASSWORD)]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM supermarkets").fetch_one(&s.app.db).await.unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn ac17d_too_many_failed_logins_blocked() {
    let s = server_lang(Mode::Dashboard, Some(Lang::Vi)).await;
    let mut browser = Browser::default();
    for _ in 0..5 {
        assert_eq!(browser.login(&s.router, "wrong-password-123").await.status, StatusCode::UNAUTHORIZED);
    }
    let sixth = browser.login(&s.router, PASSWORD).await;
    assert_eq!(sixth.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(sixth.body.contains("15 phút"));
}

#[tokio::test]
async fn ac17e_only_argon2id_hashes_stored() {
    let s = server(Mode::Dashboard).await;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users").fetch_one(&s.app.db).await.unwrap();
    assert!(hash.starts_with("$argon2id$"));
    assert!(!hash.contains(PASSWORD));
    let mut browser = Browser::default();
    browser.login(&s.router, PASSWORD).await;
    let csrf = browser.csrf(&s.router, "/admin/account").await;
    let r = browser
        .post(
            &s.router,
            "/admin/account/password",
            &[("csrf_token", &csrf), ("current", PASSWORD), ("password", "short"), ("confirm", "short")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let page = browser.get(&s.router, "/admin/account").await;
    assert!(page.body.contains("at least 12 characters"), "{}", page.body);
}

#[tokio::test]
async fn ac25_ac26_every_dashboard_screen_renders_with_data() {
    use mprimcp::services::{agent_tasks, alerts, products, supermarkets};
    let s = server_lang(Mode::Serve, Some(Lang::Vi)).await;
    let db = &s.app.db;
    let m =
        supermarkets::add(db, "Bách Hóa Xanh", "bachhoaxanh.com", Some("https://www.bachhoaxanh.com/tim-kiem?key={q}"))
            .await
            .unwrap();
    let input = products::UrlInput {
        supermarket_id: m.id.clone(),
        url: "https://www.bachhoaxanh.com/sua-tuoi/sua-th-true-milk-it-duong-110ml-thung-48-hop-rat-dai-de-test-xuong-dong".into(),
    };
    let p = products::add_product_with_urls(db, "Sữa TH True Milk Ít Đường 110ml Thùng 48 Hộp", Some("Sữa"), &[input])
        .await
        .unwrap();
    agent_tasks::queue_market(db, m.clone(), "manual").await.unwrap();
    let tasks = agent_tasks::claim_tasks(db, None, 10).await.unwrap();
    let ok = agent_tasks::PriceResult {
        task_id: tasks.tasks[0].task.id.clone(),
        status: "ok".into(),
        price: Some(399000),
        original_price: Some(450000),
        ..Default::default()
    };
    agent_tasks::submit_results(db, &[ok]).await.unwrap();
    sqlx::query("UPDATE price_records SET needs_review = 1").execute(db).await.unwrap();
    alerts::set_alert_for_product(db, p.product.id, 400000, None).await.unwrap();

    let mut browser = Browser::default();
    browser.login(&s.router, PASSWORD).await;
    let pages = [
        "/admin".to_string(),
        "/admin/supermarkets".into(),
        format!("/admin/supermarkets/{}", m.id),
        "/admin/products".into(),
        "/admin/products?q=sua%20th".into(),
        format!("/admin/products/{}", p.product.id),
        "/admin/review".into(),
        "/admin/alerts".into(),
        "/admin/notifications".into(),
        "/admin/account".into(),
    ];
    for page in &pages {
        let r = browser.get(&s.router, page).await;
        assert_eq!(r.status, StatusCode::OK, "{page}: {}", r.body);
        assert!(r.body.contains("name=\"viewport\" content=\"width=device-width, initial-scale=1\""), "{page}");
        assert!(r.body.contains("lang=\"vi\""), "{page}");
    }
    let products = browser.get(&s.router, "/admin/products").await.body;
    assert!(products.contains("399.000 ₫"), "{products}");
    let review = browser.get(&s.router, "/admin/review").await.body;
    assert!(review.contains("Chấp nhận") && review.contains("Loại bỏ"));
    let css = send(&s.router, Request::get("/static/app.css").body(Body::empty()).unwrap()).await;
    assert_eq!(css.status, StatusCode::OK);
    assert!(css.body.contains("overflow-wrap: anywhere") && css.body.contains("width: 100%"));

    let record: i64 = sqlx::query_scalar("SELECT id FROM price_records").fetch_one(db).await.unwrap();
    let csrf = browser.csrf(&s.router, "/admin/review").await;
    let r = browser.post(&s.router, &format!("/admin/prices/{record}/accept"), &[("csrf_token", &csrf)]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let flagged: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM price_records WHERE needs_review = 1").fetch_one(db).await.unwrap();
    assert_eq!(flagged, 0);
    let alert_id: i64 = sqlx::query_scalar("SELECT id FROM price_alerts").fetch_one(db).await.unwrap();
    let r = browser
        .post(&s.router, &format!("/admin/alerts/{alert_id}"), &[("csrf_token", &csrf), ("target_price", "380.000")])
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let target: i64 = sqlx::query_scalar("SELECT target_price FROM price_alerts").fetch_one(db).await.unwrap();
    assert_eq!(target, 380000);
    let r = browser
        .post(
            &s.router,
            &format!("/admin/supermarkets/{}/schedule", m.id),
            &[("csrf_token", &csrf), ("cron_expression", "0 8 * * 6,0"), ("enabled", "on")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let cron: Option<String> =
        sqlx::query_scalar("SELECT cron_expression FROM supermarkets").fetch_one(db).await.unwrap();
    assert_eq!(cron.as_deref(), Some("0 8 * * 6,0"));
}

#[tokio::test]
async fn dashboard_is_english_by_default_and_settings_switch_language() {
    let s = server(Mode::Dashboard).await;
    let mut browser = Browser::default();
    browser.login(&s.router, PASSWORD).await;
    let page = browser.get(&s.router, "/admin").await;
    assert!(page.body.contains("lang=\"en\""), "{}", page.body);
    assert!(page.body.contains("Overview") && page.body.contains("Next step: Add a supermarket"), "{}", page.body);
    let account = browser.get(&s.router, "/admin/account").await;
    assert!(account.body.contains("Settings") && account.body.contains("Asia/Ho_Chi_Minh"), "{}", account.body);

    let csrf = browser.csrf(&s.router, "/admin/account").await;
    let r = browser
        .post(
            &s.router,
            "/admin/settings",
            &[("csrf_token", &csrf), ("currency", "USD"), ("language", "vi"), ("timezone", "America/New_York")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let page = browser.get(&s.router, "/admin").await;
    assert!(page.body.contains("lang=\"vi\"") && page.body.contains("Tổng quan"), "{}", page.body);
    assert!(page.body.contains("USD · America/New_York"), "{}", page.body);

    let r = browser.post(&s.router, "/admin/settings", &[("csrf_token", &csrf), ("timezone", "Mars/Olympus")]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let account = browser.get(&s.router, "/admin/account").await;
    assert!(account.body.contains("Unknown timezone"), "{}", account.body);
}
