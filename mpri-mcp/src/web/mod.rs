// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
#![allow(clippy::result_large_err)]
pub mod layout;
pub mod pages;
pub mod session;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use rust_embed::RustEmbed;
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::auth::oauth;
use crate::auth::users::User;
use crate::web::session::{RateLimiter, Session, SessionKey, current_user};

#[derive(RustEmbed)]
#[folder = "assets/static/"]
struct Static;

#[derive(Clone)]
pub struct AppState {
    pub app: App,
    pub keys: Arc<SessionKey>,
    pub limiter: Arc<RateLimiter>,
}

impl AppState {
    pub async fn new(app: App) -> Result<Self> {
        let keys = SessionKey::load(&app.db).await?;
        Ok(Self { app, keys: Arc::new(keys), limiter: Arc::new(RateLimiter::default()) })
    }
}

pub type Page = Result<Response, Response>;

pub async fn require_user(st: &AppState, session: &Session, next: &str) -> Result<User, Response> {
    match current_user(&st.app.db, session).await {
        Some(u) => Ok(u),
        None => {
            let target = format!(
                "/admin/login?next={}",
                percent_encoding::utf8_percent_encode(next, percent_encoding::NON_ALPHANUMERIC)
            );
            Err(Redirect::to(&target).into_response())
        }
    }
}

pub fn check_csrf(session: &Session, form: &HashMap<String, String>) -> Result<(), Response> {
    if session.verify_csrf(form.get("csrf_token").map(String::as_str)) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            "Form expired or invalid: reload the page. / Biểu mẫu đã hết hạn hoặc không hợp lệ: hãy tải lại trang.",
        )
            .into_response())
    }
}

async fn security_headers(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.entry("x-content-type-options").or_insert(HeaderValue::from_static("nosniff"));
    h.entry("referrer-policy").or_insert(HeaderValue::from_static("same-origin"));
    if path == "/" || path.starts_with("/admin") || path.starts_with("/oauth") {
        h.entry("content-security-policy").or_insert(HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'none'; object-src 'none'; \
             base-uri 'none'; frame-ancestors 'none'",
        ));
        h.entry("x-frame-options").or_insert(HeaderValue::from_static("DENY"));
        h.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("no-store"));
    }
    if st.app.config.is_https() {
        h.entry("strict-transport-security").or_insert(HeaderValue::from_static("max-age=31536000"));
    }
    resp
}

fn static_file(path: &str) -> Response {
    match Static::get(path) {
        Some(file) => (
            [
                (header::CONTENT_TYPE, file.metadata.mimetype().to_string()),
                (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
            ],
            file.data.into_owned(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn static_handler(Path(path): Path<String>) -> Response {
    static_file(&path)
}

async fn health() -> Response {
    axum::Json(serde_json::json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")})).into_response()
}

fn dashboard_routes(with_consent: bool) -> Router<AppState> {
    use pages::*;
    let mut r = Router::new()
        .route("/admin/login", get(auth_pages::login_form).post(auth_pages::login_submit))
        .route("/admin/logout", post(auth_pages::logout))
        .route("/admin/account", get(auth_pages::account))
        .route("/admin/account/password", post(auth_pages::change_password))
        .route("/admin/settings", post(auth_pages::save_settings))
        .route("/admin", get(overview::overview))
        .route("/admin/supermarkets", get(markets::list).post(markets::create))
        .route("/admin/supermarkets/{id}", get(markets::detail).post(markets::update))
        .route("/admin/supermarkets/{id}/schedule", post(markets::schedule))
        .route("/admin/supermarkets/{id}/delete", post(markets::delete))
        .route("/admin/products", get(products_pages::list))
        .route("/admin/products/{id}", get(products_pages::detail).post(products_pages::update))
        .route("/admin/products/{id}/urls", post(products_pages::add_url))
        .route("/admin/products/{id}/delete", post(products_pages::delete))
        .route("/admin/urls/{id}", post(products_pages::update_url))
        .route("/admin/urls/{id}/delete", post(products_pages::delete_url))
        .route("/admin/review", get(review::list))
        .route("/admin/prices/{id}/accept", post(review::accept))
        .route("/admin/prices/{id}/reject", post(review::reject))
        .route("/admin/alerts", get(alerts_pages::list).post(alerts_pages::save))
        .route("/admin/alerts/{id}", post(alerts_pages::update))
        .route("/admin/alerts/{id}/delete", post(alerts_pages::delete))
        .route("/admin/notifications", get(notify_pages::page))
        .route("/admin/notifications/summary", post(notify_pages::summary))
        .route("/admin/notifications/{channel}", post(notify_pages::save))
        .route("/admin/notifications/{channel}/test", post(notify_pages::test));
    if with_consent {
        r = r.route("/oauth/consent", get(consent::form).post(consent::submit));
    }
    r
}

fn oauth_routes() -> Router<AppState> {
    let cors = Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth::authorization_server_metadata).options(oauth::authorization_server_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(oauth::protected_resource_metadata).options(oauth::protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth::protected_resource_metadata).options(oauth::protected_resource_metadata),
        )
        .route("/register", post(oauth::register).options(oauth::register))
        .route("/token", post(oauth::token).options(oauth::token))
        .route("/revoke", post(oauth::revoke).options(oauth::revoke))
        .layer(middleware::from_fn(oauth::cors));
    cors.route("/authorize", get(oauth::authorize_get).post(oauth::authorize_post))
}

pub fn router(st: AppState, mcp: bool, cancel: CancellationToken) -> Router {
    let dashboard = dashboard_routes(mcp).layer(middleware::from_fn_with_state(st.clone(), session::layer));
    let mut app = Router::new()
        .route("/health", get(health))
        .route("/", get(|| async { Redirect::to("/admin") }))
        .route("/favicon.ico", get(|| async { static_file("favicon.ico") }))
        .route("/apple-touch-icon.png", get(|| async { static_file("apple-touch-icon.png") }))
        .route("/static/{*path}", get(static_handler))
        .merge(dashboard);
    if mcp {
        let service = crate::mcp::http_service(st.app.clone(), cancel);
        let protected = Router::new()
            .nest_service("/mcp", service)
            .layer(middleware::from_fn_with_state(st.clone(), oauth::require_bearer));
        app = app.merge(oauth_routes()).merge(protected);
    }
    app.layer(middleware::from_fn_with_state(st.clone(), security_headers)).with_state(st)
}

pub async fn serve(st: AppState, mcp: bool, cancel: CancellationToken) -> Result<()> {
    let bind = st.app.config.bind;
    let listener = tokio::net::TcpListener::bind(bind).await.with_context(|| format!("cannot listen on {bind}"))?;
    let router = router(st, mcp, cancel.clone());
    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await?;
    Ok(())
}
