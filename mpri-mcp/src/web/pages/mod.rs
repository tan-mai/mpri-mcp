// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub mod alerts_pages;
pub mod auth_pages;
pub mod consent;
pub mod markets;
pub mod notify_pages;
pub mod overview;
pub mod products_pages;
pub mod review;

use std::convert::Infallible;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;

use crate::web::AppState;
use crate::web::session::client_ip;

pub struct ClientIp(pub String);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let addr = parts.extensions.get::<ConnectInfo<SocketAddr>>();
        Ok(ClientIp(client_ip(&parts.headers, addr, st.app.config.trust_proxy)))
    }
}

pub fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.contains('\\') => n.to_string(),
        _ => "/admin".into(),
    }
}
