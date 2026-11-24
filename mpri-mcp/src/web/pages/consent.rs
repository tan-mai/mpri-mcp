// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use maud::html;

use crate::auth::oauth::{self, Pending};
use crate::locale::{Lang, t};
use crate::prefs::load_or_default;
use crate::web::layout::{csrf_field, page, page_with_status};
use crate::web::session::Session;
use crate::web::{AppState, Page, check_csrf, require_user};

fn expired(session: &Session, lang: Lang) -> Response {
    page_with_status(
        StatusCode::BAD_REQUEST,
        t(lang, "Request expired", "Yêu cầu đã hết hạn"),
        "",
        session,
        None,
        lang,
        html! {
            div.narrow {
                h1 { (t(lang, "The connection request has expired", "Yêu cầu kết nối đã hết hạn")) }
                p { (t(lang, "This request is no longer valid. Connect again from your AI client.",
                    "Yêu cầu này không còn hiệu lực. Hãy kết nối lại từ AI client.")) }
            }
        },
    )
}

fn host_of(uri: &str) -> String {
    url::Url::parse(uri)
        .ok()
        .map(|u| match (u.host_str(), u.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_string(),
            _ => uri.to_string(),
        })
        .unwrap_or_else(|| uri.to_string())
}

pub async fn form(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<HashMap<String, String>>,
) -> Page {
    let req = q.get("req").cloned().unwrap_or_default();
    let user = require_user(&st, &session, &format!("/oauth/consent?req={req}")).await?;
    let lang = load_or_default(&st.app.db).await.language;
    let Ok(Some((pending, client))) = oauth::get_pending(&st.app.db, &req).await else {
        return Ok(expired(&session, lang));
    };
    let name = client
        .client_name
        .clone()
        .unwrap_or_else(|| t(lang, "An unnamed application", "Một ứng dụng không tên").into());
    let body = html! {
        div.narrow {
            h1 { (t(lang, "Allow access to MPriMCP?", "Cho phép truy cập MPriMCP?")) }
            div.card {
                p { b { (name) } (t(lang, " wants to use the MPriMCP tools on behalf of ", " muốn dùng các tool MPriMCP thay mặt ")) b { (user.username) } "." }
                p.muted { (t(lang, "After you allow it, the browser returns to ", "Sau khi đồng ý, trình duyệt sẽ quay về ")) code { (host_of(&pending.redirect_uri)) } "." }
                p.muted { (t(lang, "Scope: ", "Phạm vi: ")) code { (pending.scopes) } }
                form method="post" action="/oauth/consent" {
                    (csrf_field(&session))
                    input type="hidden" name="req" value=(pending.id);
                    div.actions {
                        button type="submit" name="decision" value="allow" { (t(lang, "Allow", "Cho phép")) }
                        button.secondary type="submit" name="decision" value="deny" { (t(lang, "Deny", "Từ chối")) }
                    }
                }
            }
        }
    };
    Ok(page(t(lang, "Allow access", "Cho phép truy cập"), "", &session, Some(&user), lang, body))
}

fn finish(pending: &Pending, issuer: &str, code: Option<String>) -> Response {
    let mut resp = Redirect::to(&oauth::consent_redirect(pending, issuer, code)).into_response();
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

pub async fn submit(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    let req = form.get("req").cloned().unwrap_or_default();
    let user = require_user(&st, &session, &format!("/oauth/consent?req={req}")).await?;
    let Ok(Some((pending, client))) = oauth::get_pending(&st.app.db, &req).await else {
        let lang = load_or_default(&st.app.db).await.language;
        return Ok(expired(&session, lang));
    };
    let approve = form.get("decision").map(String::as_str) == Some("allow");
    match oauth::complete_pending(&st.app.db, &pending, approve, &user.username).await {
        Ok(code) => {
            tracing::info!(client = ?client.client_name, approve, "OAuth consent");
            Ok(finish(&pending, &st.app.config.public_url, code))
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()),
    }
}
