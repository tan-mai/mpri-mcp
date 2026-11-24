// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use maud::{DOCTYPE, Markup, html};

use crate::auth::users::User;
use crate::locale::{Lang, t};
use crate::web::session::Session;

const NAV: [(&str, &str, &str); 7] = [
    ("/admin", "Overview", "Tổng quan"),
    ("/admin/supermarkets", "Supermarkets", "Siêu thị"),
    ("/admin/products", "Products", "Sản phẩm"),
    ("/admin/review", "Review", "Cần duyệt"),
    ("/admin/alerts", "Price alerts", "Mốc giá"),
    ("/admin/notifications", "Notifications", "Thông báo"),
    ("/admin/account", "Account", "Tài khoản"),
];

pub fn csrf_field(session: &Session) -> Markup {
    html! { input type="hidden" name="csrf_token" value=(session.csrf()); }
}

pub fn page(title: &str, active: &str, session: &Session, user: Option<&User>, lang: Lang, body: Markup) -> Response {
    page_with_status(StatusCode::OK, title, active, session, user, lang, body)
}

pub fn page_with_status(
    status: StatusCode,
    title: &str,
    active: &str,
    session: &Session,
    user: Option<&User>,
    lang: Lang,
    body: Markup,
) -> Response {
    let flashes = session.take_flashes();
    let markup = html! {
        (DOCTYPE)
        html lang=(lang.code()) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " · MPriMCP" }
                link rel="icon" href="/favicon.ico";
                link rel="apple-touch-icon" href="/apple-touch-icon.png";
                link rel="stylesheet" href="/static/app.css";
            }
            body {
                header {
                    div.bar {
                        a.brand href="/admin" { img src="/static/avatar.svg" alt=""; "MPriMCP" }
                        @if let Some(u) = user {
                            span.muted { (u.username) }
                            form method="post" action="/admin/logout" {
                                (csrf_field(session))
                                button.secondary type="submit" { (t(lang, "Sign out", "Đăng xuất")) }
                            }
                            nav {
                                @for (href, en, vi) in NAV {
                                    a href=(href) class=[(href == active).then_some("on")] { (t(lang, en, vi)) }
                                }
                            }
                        }
                    }
                }
                main {
                    @for (kind, message) in &flashes {
                        div class={ "flash " (kind) } { (message) }
                    }
                    (body)
                }
            }
        }
    };
    (status, markup).into_response()
}

pub fn back(session: &Session, kind: &str, message: impl Into<String>, to: &str) -> Response {
    session.flash(kind, message);
    Redirect::to(to).into_response()
}

pub fn on_off(lang: Lang, v: bool) -> &'static str {
    if v { t(lang, "on", "bật") } else { t(lang, "off", "tắt") }
}
