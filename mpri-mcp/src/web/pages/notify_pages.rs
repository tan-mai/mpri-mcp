// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Path, State};
use maud::{Markup, html};

use crate::clock::fmt_local;
use crate::services::notify::{self, ChannelConfig, TokenChange};
use crate::web::layout::{self, back, csrf_field};
use crate::web::session::Session;
use crate::web::{AppState, Page, check_csrf, require_user};

const PAGE: &str = "/admin/notifications";

fn status(c: Option<&ChannelConfig>) -> Markup {
    html! {
        @match c {
            None => p.muted { "Chưa cài đặt." },
            Some(c) => div.row {
                span { "Token: " code { (c.target_masked()) } }
                span.badge class=[c.enabled.then_some("ok")] { (if c.enabled { "bật" } else { "tắt" }) }
                @match c.last_test_ok {
                    Some(true) => span.badge.ok { "test OK " (fmt_local(c.last_test_at, "%d/%m %H:%M")) },
                    Some(false) => span.badge.bad { "test lỗi " (fmt_local(c.last_test_at, "%d/%m %H:%M")) },
                    None => span.muted { "chưa test" },
                }
            },
        }
        @if let Some(e) = c.and_then(|c| c.last_test_error.as_ref()) { p.muted { "Lỗi: " (e) } }
    }
}

pub async fn page_handler(st: &AppState, session: &Session) -> Page {
    let user = require_user(st, session, PAGE).await?;
    let db = &st.app.db;
    let telegram = notify::get_channel(db, "telegram").await.ok().flatten();
    let discord = notify::get_channel(db, "discord").await.ok().flatten();
    let settings = notify::get_settings(db).await.ok();
    let body = html! {
        h1 { "Thông báo" }
        p.muted { "Máy chủ gửi tin khi giá xuống dưới mốc và tóm tắt giá rẻ nhất sau mỗi lượt crawl. Token chỉ hiển thị dạng đã che." }
        div.card {
            h3 { "Telegram" }
            (status(telegram.as_ref()))
            form.stack method="post" action={ (PAGE) "/telegram" } {
                (csrf_field(session))
                label { "Bot token (từ @BotFather)"
                    input type="password" name="token" autocomplete="off"
                        placeholder=(if telegram.is_some() { "Để trống để giữ token đang lưu" } else { "123456789:AA…" });
                }
                label { "Chat ID" input type="text" name="chat_id" value=(telegram.as_ref().and_then(|c| c.chat_id.clone()).unwrap_or_default()) placeholder="123456789"; }
                label.check { input type="checkbox" name="enabled" checked[telegram.as_ref().is_none_or(|c| c.enabled)]; "Bật" }
                @if telegram.is_some() { label.check { input type="checkbox" name="remove"; "Xóa cấu hình Telegram" } }
                div.actions { button type="submit" { "Lưu và gửi test" } }
            }
            @if telegram.is_some() {
                form method="post" action={ (PAGE) "/telegram/test" } {
                    (csrf_field(session)) div.actions { button.secondary type="submit" { "Gửi test" } }
                }
            }
        }
        div.card {
            h3 { "Discord" }
            (status(discord.as_ref()))
            form.stack method="post" action={ (PAGE) "/discord" } {
                (csrf_field(session))
                label { "Webhook URL"
                    input type="password" name="token" autocomplete="off"
                        placeholder=(if discord.is_some() { "Để trống để giữ webhook đang lưu" } else { "https://discord.com/api/webhooks/…" });
                }
                label.check { input type="checkbox" name="enabled" checked[discord.as_ref().is_none_or(|c| c.enabled)]; "Bật" }
                @if discord.is_some() { label.check { input type="checkbox" name="remove"; "Xóa cấu hình Discord" } }
                div.actions { button type="submit" { "Lưu và gửi test" } }
            }
            @if discord.is_some() {
                form method="post" action={ (PAGE) "/discord/test" } {
                    (csrf_field(session)) div.actions { button.secondary type="submit" { "Gửi test" } }
                }
            }
        }
        div.card {
            h3 { "Tóm tắt sau mỗi lượt crawl" }
            form method="post" action={ (PAGE) "/summary" } {
                (csrf_field(session))
                label.check { input type="checkbox" name="enabled" checked[settings.as_ref().is_none_or(|s| s.summary_enabled)]; "Gửi tóm tắt giá rẻ nhất" }
                div.actions { button.secondary type="submit" { "Lưu" } }
            }
        }
    };
    Ok(layout::page("Thông báo", PAGE, session, Some(&user), body))
}

pub async fn page(State(st): State<AppState>, Extension(session): Extension<Session>) -> Page {
    page_handler(&st, &session).await
}

fn label(channel: &str) -> &'static str {
    if channel == "telegram" { "Telegram" } else { "Discord" }
}

pub async fn save(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(channel): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, PAGE).await?;
    let token = if form.contains_key("remove") {
        TokenChange::Set(String::new())
    } else {
        match form.get("token").map(|t| t.trim()).filter(|t| !t.is_empty()) {
            Some(t) => TokenChange::Set(t.to_string()),
            None => TokenChange::Keep,
        }
    };
    let enabled = form.contains_key("enabled");
    let result = st.app.notifier.set_channel(&channel, token, form.get("chat_id").map(String::as_str), enabled).await;
    Ok(match result {
        Ok(r) if r.removed => back(&session, "success", format!("Đã xóa cấu hình {}.", label(&r.channel)), PAGE),
        Ok(r) if r.test_ok => back(&session, "success", format!("Đã lưu {} và gửi tin test.", label(&r.channel)), PAGE),
        Ok(r) => back(
            &session,
            "warning",
            format!("Đã lưu {}, nhưng gửi test thất bại: {}", label(&r.channel), r.error.unwrap_or_default()),
            PAGE,
        ),
        Err(e) => back(&session, "error", format!("Chưa lưu: {e}"), PAGE),
    })
}

pub async fn test(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(channel): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, PAGE).await?;
    let Ok(channel) = notify::validate_channel(&channel) else {
        return Ok(back(&session, "error", "Kênh không hợp lệ.", PAGE));
    };
    Ok(match st.app.notifier.send_test(&channel).await {
        Ok(()) => back(&session, "success", format!("Đã gửi tin test tới {}.", label(&channel)), PAGE),
        Err(e) => back(&session, "error", format!("{}: {e}", label(&channel)), PAGE),
    })
}

pub async fn summary(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, PAGE).await?;
    let enabled = form.contains_key("enabled");
    Ok(match notify::set_summary_enabled(&st.app.db, enabled).await {
        Ok(()) => back(&session, "success", if enabled { "Đã bật tóm tắt." } else { "Đã tắt tóm tắt." }, PAGE),
        Err(e) => back(&session, "error", e.to_string(), PAGE),
    })
}
