// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::auth::users::{self, User};
use crate::clock::{HOUR_MS, now_ms};
use crate::db::Db;
use crate::web::AppState;

pub const COOKIE: &str = "mprimcp_session";
const IDLE_MS: i64 = 12 * HOUR_MS;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionData {
    #[serde(default)]
    pub uid: Option<i64>,
    #[serde(default)]
    pub sv: i64,
    pub csrf: String,
    pub exp: i64,
    #[serde(default)]
    pub flash: Vec<(String, String)>,
}

struct Inner {
    data: SessionData,
    dirty: bool,
}

#[derive(Clone)]
pub struct Session(Arc<Mutex<Inner>>);

fn random_token() -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

impl Session {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn csrf(&self) -> String {
        self.lock().data.csrf.clone()
    }

    pub fn uid(&self) -> Option<(i64, i64)> {
        let g = self.lock();
        g.data.uid.map(|u| (u, g.data.sv))
    }

    pub fn login(&self, user: &User) {
        let mut g = self.lock();
        g.data = SessionData {
            uid: Some(user.id),
            sv: user.session_version,
            csrf: random_token(),
            exp: now_ms() + IDLE_MS,
            flash: std::mem::take(&mut g.data.flash),
        };
        g.dirty = true;
    }

    pub fn logout(&self) {
        let mut g = self.lock();
        g.data = SessionData { csrf: random_token(), exp: now_ms() + IDLE_MS, ..Default::default() };
        g.dirty = true;
    }

    pub fn flash(&self, kind: &str, message: impl Into<String>) {
        let mut g = self.lock();
        g.data.flash.push((kind.to_string(), message.into()));
        g.dirty = true;
    }

    pub fn take_flashes(&self) -> Vec<(String, String)> {
        let mut g = self.lock();
        if g.data.flash.is_empty() {
            return Vec::new();
        }
        g.dirty = true;
        std::mem::take(&mut g.data.flash)
    }

    pub fn verify_csrf(&self, sent: Option<&str>) -> bool {
        let expected = self.csrf();
        let sent = sent.unwrap_or_default();
        !sent.is_empty()
            && sent.len() == expected.len()
            && sent.bytes().zip(expected.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
    }
}

pub struct SessionKey(Vec<u8>);

impl SessionKey {
    pub async fn load(db: &Db) -> anyhow::Result<Self> {
        let existing: Option<String> =
            sqlx::query_scalar("SELECT value FROM app_meta WHERE key = 'session_key'").fetch_optional(db).await?;
        if let Some(hexkey) = existing
            && let Ok(bytes) = hex::decode(hexkey)
        {
            return Ok(Self(bytes));
        }
        let mut buf = [0u8; 32];
        rand::rng().fill_bytes(&mut buf);
        sqlx::query("INSERT OR IGNORE INTO app_meta (key, value) VALUES ('session_key', ?)")
            .bind(hex::encode(buf))
            .execute(db)
            .await?;
        let stored: String =
            sqlx::query_scalar("SELECT value FROM app_meta WHERE key = 'session_key'").fetch_one(db).await?;
        Ok(Self(hex::decode(stored)?))
    }

    fn sign(&self, payload: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("hmac key");
        mac.update(payload.as_bytes());
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    }

    fn encode(&self, data: &SessionData) -> String {
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(data).unwrap_or_default());
        format!("{payload}.{}", self.sign(&payload))
    }

    fn decode(&self, value: &str) -> Option<SessionData> {
        let (payload, sig) = value.split_once('.')?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).ok()?;
        mac.update(payload.as_bytes());
        mac.verify_slice(&URL_SAFE_NO_PAD.decode(sig).ok()?).ok()?;
        let data: SessionData = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
        (data.exp > now_ms()).then_some(data)
    }
}

fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).find_map(|raw| {
        raw.split(';').find_map(|part| {
            let (k, v) = part.trim().split_once('=')?;
            (k == name).then(|| v.to_string())
        })
    })
}

pub async fn layer(State(st): State<AppState>, mut req: Request, next: Next) -> Response {
    let existing = read_cookie(req.headers(), COOKIE).and_then(|v| st.keys.decode(&v));
    let dirty = existing.is_none();
    let data =
        existing.unwrap_or_else(|| SessionData { csrf: random_token(), exp: now_ms() + IDLE_MS, ..Default::default() });
    let session = Session(Arc::new(Mutex::new(Inner { data, dirty })));
    req.extensions_mut().insert(session.clone());
    let mut resp = next.run(req).await;
    let g = session.lock();
    if g.dirty {
        let secure = if st.app.config.is_https() { "; Secure" } else { "" };
        let cookie = format!(
            "{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
            st.keys.encode(&g.data),
            IDLE_MS / 1000
        );
        if let Ok(v) = HeaderValue::from_str(&cookie) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    resp
}

pub async fn current_user(db: &Db, session: &Session) -> Option<User> {
    let (uid, sv) = session.uid()?;
    match users::get(db, uid).await.ok().flatten() {
        Some(u) if u.session_version == sv => Some(u),
        _ => {
            session.logout();
            None
        }
    }
}

pub fn client_ip(headers: &HeaderMap, addr: Option<&ConnectInfo<SocketAddr>>, trust_proxy: bool) -> String {
    if trust_proxy {
        let forwarded = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        if let Some(ip) =
            forwarded.or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()).map(str::to_string))
        {
            return ip;
        }
    }
    addr.map(|a| a.0.ip().to_string()).unwrap_or_else(|| "unknown".into())
}

pub struct RateLimiter {
    max_failures: usize,
    window: Duration,
    failures: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self { max_failures: 5, window: Duration::from_secs(15 * 60), failures: Mutex::new(HashMap::new()) }
    }
}

impl RateLimiter {
    fn prune(&self, q: &mut VecDeque<Instant>, now: Instant) {
        while q.front().is_some_and(|t| now.duration_since(*t) > self.window) {
            q.pop_front();
        }
    }

    pub fn blocked(&self, keys: &[String]) -> bool {
        let now = Instant::now();
        let mut map = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        keys.iter().any(|k| {
            let q = map.entry(k.clone()).or_default();
            self.prune(q, now);
            q.len() >= self.max_failures
        })
    }

    pub fn record_failure(&self, keys: &[String]) {
        let now = Instant::now();
        let mut map = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        for k in keys {
            let q = map.entry(k.clone()).or_default();
            self.prune(q, now);
            q.push_back(now);
        }
    }

    pub fn reset(&self, keys: &[String]) {
        let mut map = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        for k in keys {
            map.remove(k);
        }
    }
}
