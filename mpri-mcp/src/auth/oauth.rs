// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Form;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::clock::{DAY_MS, MINUTE_MS, now_ms};
use crate::db::Db;
use crate::error::SResult;
use crate::web::AppState;

pub const SCOPE: &str = "mpri";
pub const OFFLINE_ACCESS: &str = "offline_access";
pub const ACCESS_TTL_SECS: i64 = 3600;
pub const REFRESH_TTL_MS: i64 = 30 * DAY_MS;
pub const CODE_TTL_MS: i64 = 5 * MINUTE_MS;
pub const PENDING_TTL_MS: i64 = 10 * MINUTE_MS;
const CLAUDE_CALLBACKS: [&str; 2] =
    ["https://claude.ai/api/mcp/auth_callback", "https://claude.com/api/mcp/auth_callback"];
const AUTH_METHODS: [&str; 3] = ["client_secret_post", "client_secret_basic", "none"];
const JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

pub fn token_hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn random_urlsafe(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn is_loopback(uri: &str) -> bool {
    url::Url::parse(uri)
        .map(|u| u.scheme() == "http" && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1")))
        .unwrap_or(false)
}

pub fn redirect_uri_matches(registered: &str, requested: &str) -> bool {
    if registered == requested {
        return true;
    }
    if is_loopback(registered) && is_loopback(requested) {
        let (Ok(a), Ok(b)) = (url::Url::parse(registered), url::Url::parse(requested)) else { return false };
        let path = |u: &url::Url| if u.path().is_empty() { "/".to_string() } else { u.path().to_string() };
        return a.host_str() == b.host_str() && path(&a) == path(&b) && a.query() == b.query();
    }
    false
}

fn split_scopes(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

// ------------------------------------------------------------------ clients

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClientInfo {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub grant_types: Vec<String>,
    #[serde(default)]
    pub response_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id_issued_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_expires_at: Option<i64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl ClientInfo {
    fn validate_redirect(&self, requested: Option<&str>) -> Result<String, String> {
        match requested {
            Some(r) => {
                if self.redirect_uris.iter().any(|u| redirect_uri_matches(u, r)) {
                    Ok(r.to_string())
                } else {
                    Err(format!("Redirect URI '{r}' not registered for client"))
                }
            }
            None if self.redirect_uris.len() == 1 => Ok(self.redirect_uris[0].clone()),
            None => Err("redirect_uri must be specified unless the client has exactly one registered URI".into()),
        }
    }

    fn validate_scope(&self, requested: Option<&str>) -> Result<Option<Vec<String>>, String> {
        let Some(requested) = requested else { return Ok(None) };
        let mut allowed = split_scopes(self.scope.as_deref().unwrap_or_default());
        allowed.push(OFFLINE_ACCESS.into());
        let wanted = split_scopes(requested);
        let unknown: Vec<&String> = wanted.iter().filter(|s| !allowed.contains(s)).collect();
        if !unknown.is_empty() {
            let list: Vec<&str> = unknown.iter().map(|s| s.as_str()).collect();
            return Err(format!("Client was not registered with scope {}", list.join(" ")));
        }
        Ok(Some(wanted))
    }
}

pub async fn get_client(db: &Db, client_id: &str) -> SResult<Option<ClientInfo>> {
    let row: Option<(String, Option<String>, Option<i64>)> =
        sqlx::query_as("SELECT metadata, client_secret, revoked_at FROM oauth_clients WHERE client_id = ?")
            .bind(client_id)
            .fetch_optional(db)
            .await?;
    let Some((metadata, secret, revoked)) = row else { return Ok(None) };
    if revoked.is_some() {
        return Ok(None);
    }
    let mut info: ClientInfo = serde_json::from_str(&metadata).unwrap_or_default();
    info.client_id = client_id.to_string();
    info.client_secret = secret;
    Ok(Some(info))
}

fn redirect_allowed(state: &AppState, uri: &str) -> bool {
    CLAUDE_CALLBACKS.contains(&uri) || state.app.config.extra_redirect_uris.iter().any(|u| u == uri) || is_loopback(uri)
}

// ------------------------------------------------------------------ responses

fn no_store(mut resp: Response) -> Response {
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    resp
}

fn oauth_error(status: StatusCode, error: &str, description: impl Into<String>) -> Response {
    no_store((status, axum::Json(json!({"error": error, "error_description": description.into()}))).into_response())
}

fn with_query(base: &str, params: &[(&str, Option<String>)]) -> String {
    let Ok(mut url) = url::Url::parse(base) else { return base.to_string() };
    {
        let mut q = url.query_pairs_mut();
        for (k, v) in params {
            if let Some(v) = v {
                q.append_pair(k, v);
            }
        }
    }
    url.to_string()
}

fn redirect_302(location: &str) -> Response {
    no_store((StatusCode::FOUND, [(header::LOCATION, location.to_string())]).into_response())
}

pub async fn cors(req: Request, next: Next) -> Response {
    let preflight = req.method() == Method::OPTIONS;
    let mut resp = if preflight { StatusCode::OK.into_response() } else { next.run(req).await };
    let h = resp.headers_mut();
    h.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    if preflight {
        h.insert("access-control-allow-methods", HeaderValue::from_static("GET, POST, OPTIONS"));
        h.insert(
            "access-control-allow-headers",
            HeaderValue::from_static("authorization, content-type, mcp-protocol-version"),
        );
        h.insert("access-control-max-age", HeaderValue::from_static("600"));
    }
    resp
}

// ------------------------------------------------------------------ metadata

pub async fn authorization_server_metadata(State(st): State<AppState>) -> Response {
    let issuer = st.app.config.public_url.clone();
    axum::Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "registration_endpoint": format!("{issuer}/register"),
        "scopes_supported": [SCOPE, OFFLINE_ACCESS],
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": AUTH_METHODS,
        "revocation_endpoint": format!("{issuer}/revoke"),
        "revocation_endpoint_auth_methods_supported": AUTH_METHODS,
        "code_challenge_methods_supported": ["S256"],
        "authorization_response_iss_parameter_supported": true,
    }))
    .into_response()
}

pub async fn protected_resource_metadata(State(st): State<AppState>) -> Response {
    let c = &st.app.config;
    axum::Json(json!({
        "resource": c.mcp_url(),
        "authorization_servers": [c.public_url],
        "scopes_supported": [SCOPE],
        "bearer_methods_supported": ["header"],
        "resource_name": "MPriMCP",
    }))
    .into_response()
}

// ------------------------------------------------------------------ register

pub async fn register(State(st): State<AppState>, body: axum::body::Bytes) -> Response {
    let invalid = |d: String| oauth_error(StatusCode::BAD_REQUEST, "invalid_client_metadata", d);
    let mut meta: serde_json::Map<String, Value> = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(m)) => m,
        _ => return invalid("body must be a JSON object".into()),
    };
    meta.retain(|_, v| !v.is_null() && v.as_str() != Some(""));
    let redirect_uris: Vec<String> = match meta.get("redirect_uris") {
        Some(Value::Array(a)) if !a.is_empty() => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        _ => return invalid("redirect_uris: at least one redirect URI is required".into()),
    };
    if redirect_uris.iter().any(|u| url::Url::parse(u).is_err()) {
        return invalid("redirect_uris: invalid URL".into());
    }
    let method =
        meta.get("token_endpoint_auth_method").and_then(Value::as_str).unwrap_or("client_secret_post").to_string();
    if method == "private_key_jwt" {
        return invalid("token_endpoint_auth_method 'private_key_jwt' is not supported".into());
    }
    if !AUTH_METHODS.contains(&method.as_str()) {
        return invalid(format!("token_endpoint_auth_method '{method}' is not supported"));
    }
    let scope = match meta.get("scope").and_then(Value::as_str) {
        None => SCOPE.to_string(),
        Some(s) => {
            let bad: Vec<String> = split_scopes(s).into_iter().filter(|x| x != SCOPE && x != OFFLINE_ACCESS).collect();
            if !bad.is_empty() {
                return invalid(format!("Requested scopes are not valid: {}", bad.join(", ")));
            }
            s.to_string()
        }
    };
    let grant_types: Vec<String> = meta
        .get("grant_types")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_else(|| vec!["authorization_code".into(), "refresh_token".into()]);
    if !grant_types.iter().any(|g| g == "authorization_code") {
        return invalid("grant_types must include 'authorization_code'".into());
    }
    if grant_types.iter().any(|g| g == JWT_BEARER) {
        return invalid(format!(
            "grant_types must not include '{JWT_BEARER}'; the identity-assertion grant requires a pre-registered client"
        ));
    }
    let response_types: Vec<String> = meta
        .get("response_types")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_else(|| vec!["code".into()]);
    if !response_types.iter().any(|r| r == "code") {
        return invalid("response_types must include 'code' for authorization_code grant".into());
    }
    let rejected: Vec<&String> = redirect_uris.iter().filter(|u| !redirect_allowed(&st, u)).collect();
    if !rejected.is_empty() {
        let list: Vec<&str> = rejected.iter().map(|s| s.as_str()).collect();
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            format!(
                "Redirect URI not allowed: {}. Allowed: Claude's callback, MPRIMCP_OAUTH_REDIRECT_URIS and loopback addresses.",
                list.join(", ")
            ),
        );
    }
    let client_id = uuid::Uuid::new_v4().to_string();
    let issued_at = now_ms() / 1000;
    let secret = (method != "none").then(|| random_hex(32));
    for key in ["client_id", "client_secret", "client_id_issued_at", "client_secret_expires_at"] {
        meta.remove(key);
    }
    let info = ClientInfo {
        client_id: client_id.clone(),
        client_secret: None,
        client_name: meta.remove("client_name").and_then(|v| v.as_str().map(str::to_string)),
        redirect_uris: redirect_uris.clone(),
        grant_types,
        response_types,
        token_endpoint_auth_method: Some(method),
        scope: Some(scope),
        client_id_issued_at: Some(issued_at),
        client_secret_expires_at: secret.as_ref().map(|_| 0),
        extra: {
            for key in ["redirect_uris", "grant_types", "response_types", "token_endpoint_auth_method", "scope"] {
                meta.remove(key);
            }
            meta
        },
    };
    let stored = serde_json::to_string(&info).unwrap_or_else(|_| "{}".into());
    let saved = sqlx::query(
        "INSERT INTO oauth_clients (client_id, client_secret, client_name, metadata, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&client_id)
    .bind(&secret)
    .bind(&info.client_name)
    .bind(stored)
    .bind(now_ms())
    .execute(&st.app.db)
    .await;
    if let Err(e) = saved {
        tracing::error!(error = %e, "client registration failed");
        return oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "registration failed");
    }
    tracing::info!(client = ?info.client_name, %client_id, "OAuth client registered");
    let mut body = serde_json::to_value(&info).unwrap_or_default();
    if let (Some(s), Some(obj)) = (secret, body.as_object_mut()) {
        obj.insert("client_secret".into(), Value::String(s));
    }
    (StatusCode::CREATED, axum::Json(body)).into_response()
}

// ------------------------------------------------------------------ authorize

pub async fn authorize_get(State(st): State<AppState>, Query(params): Query<HashMap<String, String>>) -> Response {
    authorize(st, params).await
}

pub async fn authorize_post(State(st): State<AppState>, Form(params): Form<HashMap<String, String>>) -> Response {
    authorize(st, params).await
}

async fn authorize(st: AppState, params: HashMap<String, String>) -> Response {
    let get = |k: &str| params.get(k).map(String::as_str).filter(|v| !v.is_empty());
    let state = get("state").map(str::to_string);
    let json_error = |error: &str, description: String| {
        no_store(
            (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"error": error, "error_description": description, "state": state})),
            )
                .into_response(),
        )
    };
    let Some(client_id) = get("client_id") else {
        return json_error("invalid_request", "client_id: Field required".into());
    };
    let client = match get_client(&st.app.db, client_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return json_error("invalid_request", format!("Client ID '{client_id}' not found")),
        Err(_) => return json_error("server_error", "An unexpected error occurred".into()),
    };
    let redirect_uri = match client.validate_redirect(get("redirect_uri")) {
        Ok(r) => r,
        Err(e) => return json_error("invalid_request", e),
    };
    let redirect_error = |error: &str, description: String| {
        redirect_302(&with_query(
            &redirect_uri,
            &[("error", Some(error.into())), ("error_description", Some(description)), ("state", state.clone())],
        ))
    };
    if get("response_type") != Some("code") {
        let error = if get("response_type").is_some() { "unsupported_response_type" } else { "invalid_request" };
        return redirect_error(error, "response_type: Input should be 'code'".into());
    }
    let Some(code_challenge) = get("code_challenge") else {
        return redirect_error("invalid_request", "code_challenge: Field required".into());
    };
    if get("code_challenge_method").unwrap_or("S256") != "S256" {
        return redirect_error("invalid_request", "code_challenge_method: Input should be 'S256'".into());
    }
    let scopes = match client.validate_scope(get("scope")) {
        Ok(s) => s,
        Err(e) => return redirect_error("invalid_scope", e),
    };
    let mcp_url = st.app.config.mcp_url();
    if let Some(resource) = get("resource")
        && resource.trim_end_matches('/') != mcp_url.trim_end_matches('/')
    {
        return redirect_error("invalid_target", format!("This server only issues tokens for {mcp_url}"));
    }
    let mut scopes: Vec<String> = scopes.unwrap_or_else(|| vec![SCOPE.into()]);
    let mut seen = Vec::new();
    scopes.retain(|s| {
        let keep = !seen.contains(s);
        seen.push(s.clone());
        keep
    });
    if !scopes.iter().any(|s| s == SCOPE) {
        scopes.insert(0, SCOPE.into());
    }
    let pending_id = random_urlsafe(24);
    let now = now_ms();
    let saved = sqlx::query(
        "INSERT INTO oauth_pending (id, client_id, redirect_uri, redirect_uri_explicit, state, code_challenge, scopes, \
         resource, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&pending_id)
    .bind(&client.client_id)
    .bind(&redirect_uri)
    .bind(get("redirect_uri").is_some())
    .bind(&state)
    .bind(code_challenge)
    .bind(scopes.join(" "))
    .bind(get("resource").map(str::to_string).unwrap_or(mcp_url))
    .bind(now)
    .bind(now + PENDING_TTL_MS)
    .execute(&st.app.db)
    .await;
    if saved.is_err() {
        return redirect_error("server_error", "An unexpected error occurred".into());
    }
    redirect_302(&format!("/oauth/consent?req={pending_id}"))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Pending {
    pub id: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub redirect_uri_explicit: bool,
    pub state: Option<String>,
    pub code_challenge: String,
    pub scopes: String,
    pub resource: Option<String>,
    pub expires_at: i64,
}

pub async fn get_pending(db: &Db, id: &str) -> SResult<Option<(Pending, ClientInfo)>> {
    let row: Option<Pending> = sqlx::query_as(
        "SELECT id, client_id, redirect_uri, redirect_uri_explicit, state, code_challenge, scopes, resource, expires_at \
         FROM oauth_pending WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    let Some(p) = row.filter(|p| p.expires_at >= now_ms()) else { return Ok(None) };
    Ok(get_client(db, &p.client_id).await?.map(|c| (p, c)))
}

pub async fn complete_pending(db: &Db, pending: &Pending, approve: bool, subject: &str) -> SResult<Option<String>> {
    let mut tx = crate::db::begin(db).await?;
    sqlx::query("DELETE FROM oauth_pending WHERE id = ?").bind(&pending.id).execute(&mut *tx).await?;
    if !approve {
        tx.commit().await?;
        return Ok(None);
    }
    let code = random_urlsafe(32);
    let now = now_ms();
    sqlx::query(
        "INSERT INTO oauth_codes (code_hash, client_id, redirect_uri, redirect_uri_explicit, code_challenge, scopes, \
         resource, subject, family_id, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(token_hash(&code))
    .bind(&pending.client_id)
    .bind(&pending.redirect_uri)
    .bind(pending.redirect_uri_explicit)
    .bind(&pending.code_challenge)
    .bind(&pending.scopes)
    .bind(&pending.resource)
    .bind(subject)
    .bind(random_hex(16))
    .bind(now)
    .bind(now + CODE_TTL_MS)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(code))
}

pub fn consent_redirect(pending: &Pending, issuer: &str, code: Option<String>) -> String {
    let mut params = vec![("state", pending.state.clone()), ("iss", Some(issuer.to_string()))];
    match code {
        Some(c) => params.push(("code", Some(c))),
        None => {
            params.push(("error", Some("access_denied".into())));
            params.push(("error_description", Some("The account owner denied access.".into())));
        }
    }
    with_query(&pending.redirect_uri, &params)
}

// ------------------------------------------------------------------ token

async fn authenticate_client(
    db: &Db,
    headers: &HeaderMap,
    form: &HashMap<String, String>,
) -> Result<ClientInfo, String> {
    let client_id = form.get("client_id").filter(|c| !c.is_empty()).ok_or("Missing client_id")?;
    let client = get_client(db, client_id).await.ok().flatten().ok_or("Invalid client_id")?;
    let method = client.token_endpoint_auth_method.clone().unwrap_or_else(|| "client_secret_post".into());
    let sent: Option<String> = match method.as_str() {
        "client_secret_basic" => {
            let auth = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or_default();
            let encoded =
                auth.strip_prefix("Basic ").ok_or("Missing or invalid Basic authentication in Authorization header")?;
            let decoded = STANDARD
                .decode(encoded)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .ok_or("Invalid Basic authentication header")?;
            let (id, secret) = decoded.split_once(':').ok_or("Invalid Basic authentication header")?;
            let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().to_string();
            if decode(id) != *client_id {
                return Err("Client ID mismatch in Basic auth".into());
            }
            Some(decode(secret))
        }
        "client_secret_post" => form.get("client_secret").cloned(),
        "none" => None,
        other => return Err(format!("Unsupported auth method: {other}")),
    };
    if method != "none" && client.client_secret.is_none() {
        return Err("Client is registered for secret-based authentication but has no stored secret".into());
    }
    if let Some(stored) = &client.client_secret {
        let sent = sent.filter(|s| !s.is_empty()).ok_or("Client secret is required")?;
        let equal: bool = {
            use subtle_eq::ct_eq;
            ct_eq(stored.as_bytes(), sent.as_bytes())
        };
        if !equal {
            return Err("Invalid client_secret".into());
        }
        if let Some(exp) = client.client_secret_expires_at.filter(|e| *e > 0)
            && exp < now_ms() / 1000
        {
            return Err("Client secret has expired".into());
        }
    }
    Ok(client)
}

mod subtle_eq {
    pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

async fn issue(
    db: &Db,
    client_id: &str,
    scopes: &[String],
    resource: Option<&str>,
    subject: Option<&str>,
    family_id: &str,
    conn: &mut sqlx::SqliteConnection,
) -> SResult<Value> {
    let _ = db;
    let access = format!("mpri_at_{}", random_urlsafe(32));
    let refresh = format!("mpri_rt_{}", random_urlsafe(32));
    let now = now_ms();
    let scope_text = scopes.join(" ");
    for (token, kind, expires) in
        [(&access, "access", now + ACCESS_TTL_SECS * 1000), (&refresh, "refresh", now + REFRESH_TTL_MS)]
    {
        sqlx::query(
            "INSERT INTO oauth_tokens (token_hash, kind, client_id, scopes, resource, subject, family_id, created_at, \
             expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(token_hash(token))
        .bind(kind)
        .bind(client_id)
        .bind(&scope_text)
        .bind(resource)
        .bind(subject)
        .bind(family_id)
        .bind(now)
        .bind(expires)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query("UPDATE oauth_clients SET last_used_at = ? WHERE client_id = ?")
        .bind(now)
        .bind(client_id)
        .execute(&mut *conn)
        .await?;
    Ok(json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": ACCESS_TTL_SECS,
        "refresh_token": refresh,
        "scope": scope_text,
    }))
}

async fn revoke_family(conn: &mut sqlx::SqliteConnection, family_id: &str) -> SResult<()> {
    sqlx::query("UPDATE oauth_tokens SET revoked_at = ? WHERE family_id = ? AND revoked_at IS NULL")
        .bind(now_ms())
        .bind(family_id)
        .execute(conn)
        .await?;
    Ok(())
}

fn pkce_s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub async fn token(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let db = &st.app.db;
    let client = match authenticate_client(db, &headers, &form).await {
        Ok(c) => c,
        Err(e) => return oauth_error(StatusCode::UNAUTHORIZED, "invalid_client", e),
    };
    let bad = |error: &str, d: &str| oauth_error(StatusCode::BAD_REQUEST, error, d.to_string());
    let grant_type = form.get("grant_type").map(String::as_str).unwrap_or_default();
    match grant_type {
        "authorization_code" | "refresh_token" => {}
        JWT_BEARER => {
            return bad("unsupported_grant_type", "The JWT bearer grant is not supported by this authorization server");
        }
        _ => return bad("invalid_request", "grant_type: Input should be 'authorization_code' or 'refresh_token'"),
    }
    let grants = if client.grant_types.is_empty() {
        vec!["authorization_code".to_string(), "refresh_token".to_string()]
    } else {
        client.grant_types.clone()
    };
    if !grants.iter().any(|g| g == grant_type) {
        return bad(
            "unsupported_grant_type",
            &format!("Unsupported grant type (supported grant types are {grants:?})"),
        );
    }
    let result = if grant_type == "authorization_code" {
        exchange_code(db, &client, &form).await
    } else {
        exchange_refresh(db, &client, &form).await
    };
    match result {
        Ok(Ok(body)) => no_store(axum::Json(body).into_response()),
        Ok(Err((error, description))) => bad(error, &description),
        Err(e) => {
            tracing::error!(error = %e, "token endpoint failed");
            oauth_error(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "An unexpected error occurred")
        }
    }
}

type Grant = Result<Value, (&'static str, String)>;

#[derive(sqlx::FromRow)]
struct CodeRow {
    client_id: String,
    redirect_uri: String,
    redirect_uri_explicit: bool,
    code_challenge: String,
    scopes: String,
    resource: Option<String>,
    subject: Option<String>,
    family_id: String,
    expires_at: i64,
    used_at: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct TokenRow {
    kind: String,
    client_id: String,
    scopes: String,
    resource: Option<String>,
    subject: Option<String>,
    family_id: String,
    expires_at: i64,
    used_at: Option<i64>,
    revoked_at: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct AccessRow {
    kind: String,
    client_id: String,
    scopes: String,
    resource: Option<String>,
    subject: Option<String>,
    expires_at: i64,
    last_used_at: Option<i64>,
    client_revoked_at: Option<i64>,
}

async fn exchange_code(db: &Db, client: &ClientInfo, form: &HashMap<String, String>) -> SResult<Grant> {
    let Some(code) = form.get("code").filter(|c| !c.is_empty()) else {
        return Ok(Err(("invalid_request", "code: Field required".into())));
    };
    let Some(verifier) = form.get("code_verifier").filter(|c| !c.is_empty()) else {
        return Ok(Err(("invalid_request", "code_verifier: Field required".into())));
    };
    let mut tx = crate::db::begin(db).await?;
    let row: Option<CodeRow> = sqlx::query_as(
        "SELECT client_id, redirect_uri, redirect_uri_explicit, code_challenge, scopes, resource, subject, family_id, \
             expires_at, used_at FROM oauth_codes WHERE code_hash = ?",
    )
    .bind(token_hash(code))
    .fetch_optional(&mut *tx)
    .await?;
    let missing = Ok(Err(("invalid_grant", "authorization code does not exist".to_string())));
    let Some(CodeRow {
        client_id: cid,
        redirect_uri: redirect,
        redirect_uri_explicit: explicit,
        code_challenge: challenge,
        scopes,
        resource,
        subject,
        family_id: family,
        expires_at: expires,
        used_at: used,
    }) = row
    else {
        return missing;
    };
    if cid != client.client_id {
        return missing;
    }
    if used.is_some() {
        revoke_family(&mut tx, &family).await?;
        tx.commit().await?;
        tracing::warn!(client = %client.client_id, "authorization code replay; family revoked");
        return missing;
    }
    if expires < now_ms() {
        return Ok(Err(("invalid_grant", "authorization code has expired".into())));
    }
    let expected = explicit.then_some(redirect);
    if form.get("redirect_uri").filter(|r| !r.is_empty()).cloned() != expected {
        return Ok(Err(("invalid_request", "redirect_uri did not match the one used when creating auth code".into())));
    }
    if pkce_s256(verifier) != challenge {
        return Ok(Err(("invalid_grant", "incorrect code_verifier".into())));
    }
    let claimed = sqlx::query("UPDATE oauth_codes SET used_at = ? WHERE code_hash = ? AND used_at IS NULL")
        .bind(now_ms())
        .bind(token_hash(code))
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if claimed != 1 {
        return Ok(Err(("invalid_grant", "authorization code already used".into())));
    }
    let body =
        issue(db, &client.client_id, &split_scopes(&scopes), resource.as_deref(), subject.as_deref(), &family, &mut tx)
            .await?;
    tx.commit().await?;
    Ok(Ok(body))
}

async fn exchange_refresh(db: &Db, client: &ClientInfo, form: &HashMap<String, String>) -> SResult<Grant> {
    let Some(refresh) = form.get("refresh_token").filter(|c| !c.is_empty()) else {
        return Ok(Err(("invalid_request", "refresh_token: Field required".into())));
    };
    let mut tx = crate::db::begin(db).await?;
    let row: Option<TokenRow> = sqlx::query_as(
        "SELECT kind, client_id, scopes, resource, subject, family_id, expires_at, used_at, revoked_at \
             FROM oauth_tokens WHERE token_hash = ?",
    )
    .bind(token_hash(refresh))
    .fetch_optional(&mut *tx)
    .await?;
    let missing = Ok(Err(("invalid_grant", "refresh token does not exist".to_string())));
    let Some(TokenRow {
        kind,
        client_id: cid,
        scopes,
        resource,
        subject,
        family_id: family,
        expires_at: expires,
        used_at: used,
        revoked_at: revoked,
    }) = row
    else {
        return missing;
    };
    if kind != "refresh" || cid != client.client_id || revoked.is_some() {
        return missing;
    }
    if used.is_some() {
        revoke_family(&mut tx, &family).await?;
        tx.commit().await?;
        tracing::warn!(client = %client.client_id, "refresh token reuse; family revoked");
        return missing;
    }
    if expires < now_ms() {
        return Ok(Err(("invalid_grant", "refresh token has expired".into())));
    }
    let granted = split_scopes(&scopes);
    let requested = match form.get("scope").filter(|s| !s.is_empty()) {
        Some(s) => s.split(' ').map(str::to_string).collect(),
        None => granted.clone(),
    };
    if let Some(bad) = requested.iter().find(|s| !granted.contains(s)) {
        return Ok(Err(("invalid_scope", format!("cannot request scope `{bad}` not provided by refresh token"))));
    }
    let claimed = sqlx::query(
        "UPDATE oauth_tokens SET used_at = ? WHERE token_hash = ? AND used_at IS NULL AND revoked_at IS NULL",
    )
    .bind(now_ms())
    .bind(token_hash(refresh))
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if claimed != 1 {
        return Ok(Err(("invalid_grant", "refresh token already used".into())));
    }
    let body =
        issue(db, &client.client_id, &requested, resource.as_deref(), subject.as_deref(), &family, &mut tx).await?;
    tx.commit().await?;
    Ok(Ok(body))
}

// ------------------------------------------------------------------ revoke

pub async fn revoke(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let db = &st.app.db;
    let client = match authenticate_client(db, &headers, &form).await {
        Ok(c) => c,
        Err(e) => return oauth_error(StatusCode::UNAUTHORIZED, "unauthorized_client", e),
    };
    let Some(token) = form.get("token").filter(|t| !t.is_empty()) else {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", "token: Field required");
    };
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT client_id, family_id FROM oauth_tokens WHERE token_hash = ? AND revoked_at IS NULL")
            .bind(token_hash(token))
            .fetch_optional(db)
            .await
            .unwrap_or(None);
    if let Some((cid, family)) = row.filter(|(cid, _)| *cid == client.client_id) {
        let _ = cid;
        if let Ok(mut conn) = db.acquire().await {
            let _ = revoke_family(&mut conn, &family).await;
        }
    }
    no_store(StatusCode::OK.into_response())
}

// ------------------------------------------------------------------ resource server

#[derive(Debug, Clone)]
pub struct AccessToken {
    pub client_id: String,
    pub scopes: Vec<String>,
    pub resource: Option<String>,
    pub subject: Option<String>,
}

pub async fn load_access_token(db: &Db, token: &str) -> SResult<Option<AccessToken>> {
    let now = now_ms();
    let row: Option<AccessRow> =
        sqlx::query_as(
            "SELECT t.kind, t.client_id, t.scopes, t.resource, t.subject, t.expires_at, t.last_used_at,              c.revoked_at AS client_revoked_at \
             FROM oauth_tokens t JOIN oauth_clients c ON c.client_id = t.client_id \
             WHERE t.token_hash = ? AND t.revoked_at IS NULL",
        )
        .bind(token_hash(token))
        .fetch_optional(db)
        .await?;
    let Some(AccessRow {
        kind,
        client_id,
        scopes,
        resource,
        subject,
        expires_at: expires,
        last_used_at: last_used,
        client_revoked_at: client_revoked,
    }) = row
    else {
        return Ok(None);
    };
    if kind != "access" || expires < now || client_revoked.is_some() {
        return Ok(None);
    }
    if last_used.is_none_or(|t| now - t > MINUTE_MS) {
        sqlx::query("UPDATE oauth_tokens SET last_used_at = ? WHERE token_hash = ?")
            .bind(now)
            .bind(token_hash(token))
            .execute(db)
            .await?;
        sqlx::query("UPDATE oauth_clients SET last_used_at = ? WHERE client_id = ?")
            .bind(now)
            .bind(&client_id)
            .execute(db)
            .await?;
    }
    Ok(Some(AccessToken { client_id, scopes: split_scopes(&scopes), resource, subject }))
}

fn same_resource(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        url::Url::parse(s).map(|u| u.to_string()).unwrap_or_else(|_| s.to_string()).trim_end_matches('/').to_string()
    };
    norm(a) == norm(b)
}

fn auth_challenge(st: &AppState, status: StatusCode, error: &str, description: &str) -> Response {
    let metadata = format!("{}/.well-known/oauth-protected-resource/mcp", st.app.config.public_url);
    let www =
        format!("Bearer error=\"{error}\", error_description=\"{description}\", resource_metadata=\"{metadata}\"");
    let mut resp = (status, axum::Json(json!({"error": error, "error_description": description}))).into_response();
    if let Ok(v) = HeaderValue::from_str(&www) {
        resp.headers_mut().insert(header::WWW_AUTHENTICATE, v);
    }
    resp
}

pub async fn require_bearer(State(st): State<AppState>, req: Request<Body>, next: Next) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() > 7 && v[..7].eq_ignore_ascii_case("bearer "))
        .map(|v| v[7..].to_string());
    let Some(token) = token else {
        return auth_challenge(&st, StatusCode::UNAUTHORIZED, "invalid_token", "Authentication required");
    };
    let access = match load_access_token(&st.app.db, &token).await {
        Ok(Some(a)) => a,
        Ok(None) => return auth_challenge(&st, StatusCode::UNAUTHORIZED, "invalid_token", "Authentication required"),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if !access.resource.as_deref().is_some_and(|r| same_resource(r, &st.app.config.mcp_url())) {
        tracing::warn!(resource = ?access.resource, "bearer token issued for another resource");
        return auth_challenge(&st, StatusCode::UNAUTHORIZED, "invalid_token", "Authentication required");
    }
    if !access.scopes.iter().any(|s| s == SCOPE) {
        return auth_challenge(&st, StatusCode::FORBIDDEN, "insufficient_scope", &format!("Required scope: {SCOPE}"));
    }
    next.run(req).await
}

// ------------------------------------------------------------------ housekeeping

pub async fn cleanup_expired(db: &Db) -> SResult<()> {
    let now = now_ms();
    let cutoff = now - DAY_MS;
    sqlx::query("DELETE FROM oauth_pending WHERE expires_at < ?").bind(now).execute(db).await?;
    sqlx::query("DELETE FROM oauth_codes WHERE expires_at < ?").bind(cutoff).execute(db).await?;
    sqlx::query("DELETE FROM oauth_tokens WHERE expires_at < ?1 OR revoked_at < ?1").bind(cutoff).execute(db).await?;
    Ok(())
}

pub fn redirect_to(location: &str) -> Response {
    Redirect::to(location).into_response()
}
