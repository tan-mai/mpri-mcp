// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use maud::html;

use crate::auth::passwords::{MIN_PASSWORD_LENGTH, validate_new_password, verify_password};
use crate::auth::users;
use crate::web::layout::{back, csrf_field, page, page_with_status};
use crate::web::pages::{ClientIp, safe_next};
use crate::web::session::{Session, current_user};
use crate::web::{AppState, Page, check_csrf, require_user};

fn login_page(
    session: &Session,
    status: StatusCode,
    next: &str,
    username: &str,
    error: Option<&str>,
) -> axum::response::Response {
    page_with_status(
        status,
        "Đăng nhập",
        "",
        session,
        None,
        html! {
            div.narrow {
                h1 { "Đăng nhập" }
                @if let Some(e) = error { div.flash.error { (e) } }
                div.card {
                    form.stack method="post" action="/admin/login" {
                        (csrf_field(session))
                        input type="hidden" name="next" value=(next);
                        label { "Tên đăng nhập" input type="text" name="username" value=(username) autocomplete="username" required; }
                        label { "Mật khẩu" input type="password" name="password" autocomplete="current-password" required; }
                        div.actions { button type="submit" { "Đăng nhập" } }
                    }
                }
                p.muted { "Quên mật khẩu? Chạy " code { "mprimcp user set-password <tên>" } " trên máy chủ." }
            }
        },
    )
}

pub async fn login_form(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<HashMap<String, String>>,
) -> Page {
    let next = safe_next(q.get("next").map(String::as_str));
    if current_user(&st.app.db, &session).await.is_some() {
        return Ok(Redirect::to(&next).into_response());
    }
    Ok(login_page(&session, StatusCode::OK, &next, "", None))
}

pub async fn login_submit(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    ClientIp(ip): ClientIp,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    let username = form.get("username").map(|s| s.trim().to_string()).unwrap_or_default();
    let password = form.get("password").cloned().unwrap_or_default();
    let next = safe_next(form.get("next").map(String::as_str));
    let keys = vec![format!("ip:{ip}"), format!("user:{}", username.to_lowercase())];
    if st.limiter.blocked(&keys) {
        tracing::warn!(%ip, "login blocked after too many failures");
        return Ok(login_page(
            &session,
            StatusCode::TOO_MANY_REQUESTS,
            &next,
            &username,
            Some("Quá nhiều lần đăng nhập sai. Hãy đợi 15 phút rồi thử lại."),
        ));
    }
    match users::authenticate(&st.app.db, &username, &password).await {
        Ok(Some(user)) => {
            st.limiter.reset(&keys);
            session.login(&user);
            tracing::info!(username = %user.username, "signed in");
            Ok(Redirect::to(&next).into_response())
        }
        _ => {
            st.limiter.record_failure(&keys);
            tracing::warn!(%ip, "failed sign-in");
            Ok(login_page(
                &session,
                StatusCode::UNAUTHORIZED,
                &next,
                &username,
                Some("Sai tên đăng nhập hoặc mật khẩu."),
            ))
        }
    }
}

pub async fn logout(Extension(session): Extension<Session>, Form(form): Form<HashMap<String, String>>) -> Page {
    check_csrf(&session, &form)?;
    session.logout();
    Ok(Redirect::to("/admin/login").into_response())
}

pub async fn account(State(st): State<AppState>, Extension(session): Extension<Session>) -> Page {
    let user = require_user(&st, &session, "/admin/account").await?;
    Ok(page(
        "Tài khoản",
        "/admin/account",
        &session,
        Some(&user),
        html! {
            h1 { "Đổi mật khẩu" }
            div.card {
                form.stack method="post" action="/admin/account/password" {
                    (csrf_field(&session))
                    label { "Mật khẩu hiện tại" input type="password" name="current" autocomplete="current-password" required; }
                    label { "Mật khẩu mới (tối thiểu " (MIN_PASSWORD_LENGTH) " ký tự)" input type="password" name="password" autocomplete="new-password" required; }
                    label { "Nhập lại mật khẩu mới" input type="password" name="confirm" autocomplete="new-password" required; }
                    div.actions { button type="submit" { "Đổi mật khẩu" } }
                }
                p.muted { "Đổi mật khẩu sẽ đăng xuất mọi phiên khác." }
            }
        },
    ))
}

pub async fn change_password(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    let user = require_user(&st, &session, "/admin/account").await?;
    let get = |k: &str| form.get(k).cloned().unwrap_or_default();
    if !verify_password(Some(&user.password_hash), &get("current")) {
        return Ok(back(&session, "error", "Mật khẩu hiện tại không đúng.", "/admin/account"));
    }
    if let Some(e) = validate_new_password(&get("password"), &get("confirm"), &user.username) {
        return Ok(back(&session, "error", e, "/admin/account"));
    }
    match users::set_password(&st.app.db, &user.username, &get("password")).await {
        Ok(updated) => {
            session.login(&updated);
            Ok(back(&session, "success", "Đã đổi mật khẩu. Các phiên khác đã bị đăng xuất.", "/admin/account"))
        }
        Err(e) => Ok(back(&session, "error", e.to_string(), "/admin/account")),
    }
}
