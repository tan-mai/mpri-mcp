// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Path, State};
use maud::{Markup, html};

use crate::clock::{fmt_local, short_datetime};
use crate::locale::{Lang, t, tf};
use crate::prefs::{Prefs, load_or_default};
use crate::services::notify::{self, ChannelConfig, TokenChange};
use crate::web::layout::{self, back, csrf_field, on_off};
use crate::web::session::Session;
use crate::web::{AppState, Page, check_csrf, require_user};

const PAGE: &str = "/admin/notifications";

fn status(c: Option<&ChannelConfig>, pr: &Prefs) -> Markup {
    let lang = pr.language;
    let when = |at| fmt_local(at, short_datetime(lang), pr.timezone);
    html! {
        @match c {
            None => p.muted { (t(lang, "Not set up.", "Chưa cài đặt.")) },
            Some(c) => div.row {
                span { "Token: " code { (c.target_masked()) } }
                span.badge class=[c.enabled.then_some("ok")] { (on_off(lang, c.enabled)) }
                @match c.last_test_ok {
                    Some(true) => span.badge.ok { (t(lang, "test OK ", "test OK ")) (when(c.last_test_at)) },
                    Some(false) => span.badge.bad { (t(lang, "test failed ", "test lỗi ")) (when(c.last_test_at)) },
                    None => span.muted { (t(lang, "not tested", "chưa test")) },
                }
            },
        }
        @if let Some(e) = c.and_then(|c| c.last_test_error.as_ref()) { p.muted { (t(lang, "Error: ", "Lỗi: ")) (e) } }
    }
}

pub async fn page(State(st): State<AppState>, Extension(session): Extension<Session>) -> Page {
    let user = require_user(&st, &session, PAGE).await?;
    let db = &st.app.db;
    let pr = load_or_default(db).await;
    let lang = pr.language;
    let telegram = notify::get_channel(db, "telegram").await.ok().flatten();
    let discord = notify::get_channel(db, "discord").await.ok().flatten();
    let settings = notify::get_settings(db).await.ok();
    let body = html! {
        h1 { (t(lang, "Notifications", "Thông báo")) }
        p.muted { (t(lang,
            "The server messages you when a price drops below a target and, after every crawl, where each product is cheapest. Tokens are only shown masked.",
            "Máy chủ gửi tin khi giá xuống dưới mốc và tóm tắt giá rẻ nhất sau mỗi lượt crawl. Token chỉ hiển thị dạng đã che.")) }
        div.card {
            h3 { "Telegram" }
            (status(telegram.as_ref(), &pr))
            form.stack method="post" action={ (PAGE) "/telegram" } {
                (csrf_field(&session))
                label { (t(lang, "Bot token (from @BotFather)", "Bot token (từ @BotFather)"))
                    input type="password" name="token" autocomplete="off"
                        placeholder=(if telegram.is_some() { t(lang, "Leave empty to keep the saved token", "Để trống để giữ token đang lưu") } else { "123456789:AA…" });
                }
                label { "Chat ID" input type="text" name="chat_id" value=(telegram.as_ref().and_then(|c| c.chat_id.clone()).unwrap_or_default()) placeholder="123456789"; }
                p.muted { (t(lang,
                    "Your own chat ID from getUpdates, not the number at the start of the bot token.",
                    "Chat ID của bạn lấy từ getUpdates, không phải dãy số ở đầu bot token.")) }
                label.check { input type="checkbox" name="enabled" checked[telegram.as_ref().is_none_or(|c| c.enabled)]; (t(lang, "On", "Bật")) }
                @if telegram.is_some() { label.check { input type="checkbox" name="remove"; (t(lang, "Remove the Telegram setup", "Xoá cấu hình Telegram")) } }
                div.actions { button type="submit" { (t(lang, "Save and send a test", "Lưu và gửi test")) } }
            }
            @if telegram.is_some() {
                form method="post" action={ (PAGE) "/telegram/test" } {
                    (csrf_field(&session)) div.actions { button.secondary type="submit" { (t(lang, "Send a test", "Gửi test")) } }
                }
            }
        }
        div.card {
            h3 { "Discord" }
            (status(discord.as_ref(), &pr))
            form.stack method="post" action={ (PAGE) "/discord" } {
                (csrf_field(&session))
                label { "Webhook URL"
                    input type="password" name="token" autocomplete="off"
                        placeholder=(if discord.is_some() { t(lang, "Leave empty to keep the saved webhook", "Để trống để giữ webhook đang lưu") } else { "https://discord.com/api/webhooks/…" });
                }
                label.check { input type="checkbox" name="enabled" checked[discord.as_ref().is_none_or(|c| c.enabled)]; (t(lang, "On", "Bật")) }
                @if discord.is_some() { label.check { input type="checkbox" name="remove"; (t(lang, "Remove the Discord setup", "Xoá cấu hình Discord")) } }
                div.actions { button type="submit" { (t(lang, "Save and send a test", "Lưu và gửi test")) } }
            }
            @if discord.is_some() {
                form method="post" action={ (PAGE) "/discord/test" } {
                    (csrf_field(&session)) div.actions { button.secondary type="submit" { (t(lang, "Send a test", "Gửi test")) } }
                }
            }
        }
        div.card {
            h3 { (t(lang, "Summary after every crawl", "Tóm tắt sau mỗi lượt crawl")) }
            form method="post" action={ (PAGE) "/summary" } {
                (csrf_field(&session))
                label.check {
                    input type="checkbox" name="enabled" checked[settings.as_ref().is_none_or(|s| s.summary_enabled)];
                    (t(lang, "Send the cheapest-price summary", "Gửi tóm tắt giá rẻ nhất"))
                }
                div.actions { button.secondary type="submit" { (t(lang, "Save", "Lưu")) } }
            }
        }
    };
    Ok(layout::page(t(lang, "Notifications", "Thông báo"), PAGE, &session, Some(&user), lang, body))
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
    let lang = load_or_default(&st.app.db).await.language;
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
        Ok(r) => {
            let name = label(&r.channel);
            let (kind, msg) = if r.removed {
                ("success", tf(lang, format!("{name} setup removed."), format!("Đã xoá cấu hình {name}.")))
            } else if r.test_ok {
                (
                    "success",
                    tf(
                        lang,
                        format!("{name} saved and a test message sent."),
                        format!("Đã lưu {name} và gửi tin test."),
                    ),
                )
            } else {
                let err = r.error.unwrap_or_default();
                (
                    "warning",
                    tf(
                        lang,
                        format!("{name} saved, but the test message failed: {err}"),
                        format!("Đã lưu {name}, nhưng gửi test thất bại: {err}"),
                    ),
                )
            };
            back(&session, kind, msg, PAGE)
        }
        Err(e) => back(&session, "error", tf(lang, format!("Not saved: {e}"), format!("Chưa lưu: {e}")), PAGE),
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
    let lang = load_or_default(&st.app.db).await.language;
    let Ok(channel) = notify::validate_channel(&channel) else {
        return Ok(back(&session, "error", t(lang, "Unknown channel.", "Kênh không hợp lệ."), PAGE));
    };
    let name = label(&channel);
    Ok(match st.app.notifier.send_test(&channel).await {
        Ok(()) => back(
            &session,
            "success",
            tf(lang, format!("Test message sent to {name}."), format!("Đã gửi tin test tới {name}.")),
            PAGE,
        ),
        Err(e) => back(&session, "error", format!("{name}: {e}"), PAGE),
    })
}

pub async fn summary(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, PAGE).await?;
    let lang: Lang = load_or_default(&st.app.db).await.language;
    let enabled = form.contains_key("enabled");
    Ok(match notify::set_summary_enabled(&st.app.db, enabled).await {
        Ok(()) => back(
            &session,
            "success",
            if enabled {
                t(lang, "Summary on.", "Đã bật tóm tắt.")
            } else {
                t(lang, "Summary off.", "Đã tắt tóm tắt.")
            },
            PAGE,
        ),
        Err(e) => back(&session, "error", e.to_string(), PAGE),
    })
}
