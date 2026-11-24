// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use maud::html;

use crate::auth::passwords::{MIN_PASSWORD_LENGTH, validate_new_password, verify_password};
use crate::auth::users;
use crate::locale::{Currency, Lang, t, tf};
use crate::prefs::{self, load_or_default};
use crate::services::analytics::has_any_price;
use crate::web::layout::{back, csrf_field, page, page_with_status};
use crate::web::pages::{ClientIp, safe_next};
use crate::web::session::{Session, current_user};
use crate::web::{AppState, Page, check_csrf, require_user};

fn login_page(
    session: &Session,
    lang: Lang,
    status: StatusCode,
    next: &str,
    username: &str,
    error: Option<&str>,
) -> Response {
    page_with_status(
        status,
        t(lang, "Sign in", "Đăng nhập"),
        "",
        session,
        None,
        lang,
        html! {
            div.narrow {
                h1 { (t(lang, "Sign in", "Đăng nhập")) }
                @if let Some(e) = error { div.flash.error { (e) } }
                div.card {
                    form.stack method="post" action="/admin/login" {
                        (csrf_field(session))
                        input type="hidden" name="next" value=(next);
                        label { (t(lang, "Username", "Tên đăng nhập")) input type="text" name="username" value=(username) autocomplete="username" required; }
                        label { (t(lang, "Password", "Mật khẩu")) input type="password" name="password" autocomplete="current-password" required; }
                        div.actions { button type="submit" { (t(lang, "Sign in", "Đăng nhập")) } }
                    }
                }
                p.muted {
                    (t(lang, "Forgot the password? Run ", "Quên mật khẩu? Chạy "))
                    code { "mprimcp user set-password <username>" }
                    (t(lang, " on the server.", " trên máy chủ."))
                }
            }
        },
    )
}

pub async fn login_form(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<HashMap<String, String>>,
) -> Page {
    let lang = load_or_default(&st.app.db).await.language;
    let next = safe_next(q.get("next").map(String::as_str));
    if current_user(&st.app.db, &session).await.is_some() {
        return Ok(Redirect::to(&next).into_response());
    }
    Ok(login_page(&session, lang, StatusCode::OK, &next, "", None))
}

pub async fn login_submit(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    ClientIp(ip): ClientIp,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    let lang = load_or_default(&st.app.db).await.language;
    let username = form.get("username").map(|s| s.trim().to_string()).unwrap_or_default();
    let password = form.get("password").cloned().unwrap_or_default();
    let next = safe_next(form.get("next").map(String::as_str));
    let keys = vec![format!("ip:{ip}"), format!("user:{}", username.to_lowercase())];
    if st.limiter.blocked(&keys) {
        tracing::warn!(%ip, "login blocked after too many failures");
        return Ok(login_page(
            &session,
            lang,
            StatusCode::TOO_MANY_REQUESTS,
            &next,
            &username,
            Some(t(
                lang,
                "Too many failed attempts. Wait 15 minutes and try again.",
                "Quá nhiều lần đăng nhập sai. Hãy đợi 15 phút rồi thử lại.",
            )),
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
                lang,
                StatusCode::UNAUTHORIZED,
                &next,
                &username,
                Some(t(lang, "Wrong username or password.", "Sai tên đăng nhập hoặc mật khẩu.")),
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
    let pr = load_or_default(&st.app.db).await;
    let lang = pr.language;
    let config = &st.app.config;
    let currency_locked = config.currency.is_some() || has_any_price(&st.app.db).await.unwrap_or(true);
    let body = html! {
        h1 { (t(lang, "Account", "Tài khoản")) }
        h2 { (t(lang, "Settings", "Cài đặt")) }
        div.card {
            form.stack method="post" action="/admin/settings" {
                (csrf_field(&session))
                label { (t(lang, "Currency", "Tiền tệ"))
                    select name="currency" disabled[currency_locked] {
                        @for c in Currency::ALL { option value=(c.code()) selected[c == pr.currency] { (c.code()) } }
                    }
                }
                @if currency_locked {
                    p.muted { (t(lang,
                        "The currency is fixed: prices are already stored, or MPRIMCP_CURRENCY is set.",
                        "Tiền tệ đã cố định: đã có giá được lưu, hoặc máy chủ đặt MPRIMCP_CURRENCY.")) }
                }
                label { (t(lang, "Language of the dashboard and notifications", "Ngôn ngữ của trang quản trị và thông báo"))
                    select name="language" disabled[config.language.is_some()] {
                        @for l in Lang::ALL { option value=(l.code()) selected[l == lang] { (l.name()) } }
                    }
                }
                label { (t(lang, "Timezone (IANA name)", "Múi giờ (tên IANA)"))
                    input type="text" name="timezone" value=(pr.tz_name()) placeholder="Asia/Ho_Chi_Minh" disabled[config.timezone.is_some()];
                }
                div.actions { button type="submit" { (t(lang, "Save settings", "Lưu cài đặt")) } }
            }
        }
        h2 { (t(lang, "Change password", "Đổi mật khẩu")) }
        div.card {
            form.stack method="post" action="/admin/account/password" {
                (csrf_field(&session))
                label { (t(lang, "Current password", "Mật khẩu hiện tại")) input type="password" name="current" autocomplete="current-password" required; }
                label { (tf(lang, format!("New password (at least {MIN_PASSWORD_LENGTH} characters)"), format!("Mật khẩu mới (tối thiểu {MIN_PASSWORD_LENGTH} ký tự)")))
                    input type="password" name="password" autocomplete="new-password" required; }
                label { (t(lang, "Repeat the new password", "Nhập lại mật khẩu mới")) input type="password" name="confirm" autocomplete="new-password" required; }
                div.actions { button type="submit" { (t(lang, "Change password", "Đổi mật khẩu")) } }
            }
            p.muted { (t(lang, "Changing the password signs out every other session.", "Đổi mật khẩu sẽ đăng xuất mọi phiên khác.")) }
        }
    };
    Ok(page(t(lang, "Account", "Tài khoản"), "/admin/account", &session, Some(&user), lang, body))
}

pub async fn change_password(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    let user = require_user(&st, &session, "/admin/account").await?;
    let lang = load_or_default(&st.app.db).await.language;
    let get = |k: &str| form.get(k).cloned().unwrap_or_default();
    if !verify_password(Some(&user.password_hash), &get("current")) {
        return Ok(back(
            &session,
            "error",
            t(lang, "The current password is wrong.", "Mật khẩu hiện tại không đúng."),
            "/admin/account",
        ));
    }
    if let Some(e) = validate_new_password(&get("password"), &get("confirm"), &user.username) {
        return Ok(back(&session, "error", e, "/admin/account"));
    }
    match users::set_password(&st.app.db, &user.username, &get("password")).await {
        Ok(updated) => {
            session.login(&updated);
            Ok(back(
                &session,
                "success",
                t(
                    lang,
                    "Password changed. Other sessions were signed out.",
                    "Đã đổi mật khẩu. Các phiên khác đã bị đăng xuất.",
                ),
                "/admin/account",
            ))
        }
        Err(e) => Ok(back(&session, "error", e.to_string(), "/admin/account")),
    }
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub async fn save_settings(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, "/admin/account").await?;
    let changes = prefs::Changes {
        currency: form.get("currency").and_then(|c| Currency::parse(c)),
        language: form.get("language").and_then(|l| Lang::parse(l)),
        timezone: form.get("timezone").cloned(),
    };
    Ok(match prefs::update(&st.app.db, &st.app.config, changes).await {
        Ok(pr) => back(&session, "success", t(pr.language, "Settings saved.", "Đã lưu cài đặt."), "/admin/account"),
        Err(e) => back(&session, "error", e.to_string(), "/admin/account"),
    })
}
