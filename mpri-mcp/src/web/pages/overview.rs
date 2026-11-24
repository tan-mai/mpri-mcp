// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use axum::Extension;
use axum::extract::State;
use maud::html;

use crate::clock::{ago, fmt_local, long_datetime, short_datetime};
use crate::locale::{Lang, t};
use crate::prefs::load_or_default;
use crate::services::{agent_tasks, reviews, setup_status, supermarkets};
use crate::web::layout::{on_off, page};
use crate::web::session::Session;
use crate::web::{AppState, Page, require_user};

fn step_label(step: &str, lang: Lang) -> &'static str {
    match step {
        "add_supermarket" => t(lang, "Add a supermarket", "Thêm siêu thị"),
        "add_product" => t(lang, "Add a product", "Thêm sản phẩm"),
        "set_alerts" => t(lang, "Set price targets", "Đặt mốc giá"),
        "notifications" => t(lang, "Set up notifications", "Cài kênh thông báo"),
        "schedule" => t(lang, "Schedule price checks", "Đặt lịch kiểm tra giá"),
        "first_crawl" => t(lang, "Run the first crawl", "Chạy crawl đầu tiên"),
        _ => t(lang, "Setup is complete", "Đã thiết lập xong"),
    }
}

pub async fn overview(State(st): State<AppState>, Extension(session): Extension<Session>) -> Page {
    let user = require_user(&st, &session, "/admin").await?;
    let db = &st.app.db;
    let pr = load_or_default(db).await;
    let (lang, tz) = (pr.language, pr.timezone);
    let _ = agent_tasks::maintain(db).await;
    let status = setup_status::status(db).await.map_err(|e| e.to_string());
    let summaries = supermarkets::list_summaries(db).await.unwrap_or_default();
    let pending = agent_tasks::pending_total(db).await.unwrap_or(0);
    let review_count = reviews::count(db).await.unwrap_or(0);
    let mcp = st.app.mode == crate::config::Mode::Serve;
    let body = html! {
        h1 { (t(lang, "Overview", "Tổng quan")) }
        @match &status {
            Err(e) => div.flash.error { (e) },
            Ok(s) => {
                div.stats {
                    div.stat { b { (s.supermarkets.len()) } span.muted { (t(lang, "supermarkets", "siêu thị")) } }
                    div.stat { b { (s.products) } span.muted { (t(lang, "products", "sản phẩm")) } }
                    div.stat { b { (s.price_alerts) } span.muted { (t(lang, "price alerts", "mốc giá")) } }
                    div.stat { b { (pending) } span.muted { (t(lang, "tasks waiting for the AI", "task chờ AI đọc giá")) } }
                    div.stat { b { (review_count) } span.muted { (t(lang, "prices to review", "giá cần duyệt")) } }
                }
                div.card {
                    h3 { (t(lang, "Next step: ", "Bước tiếp theo: ")) (step_label(s.next_step, lang)) }
                    div.row.muted {
                        span { (t(lang, "Products without URLs: ", "Sản phẩm chưa có URL: ")) (s.products_without_urls) }
                        span { (t(lang, "Prices stored: ", "Đã có giá: ")) (if s.has_any_price { t(lang, "yes", "có") } else { t(lang, "not yet", "chưa") }) }
                        span { (t(lang, "Server cron: ", "Cron trên máy chủ: ")) (on_off(lang, st.app.server_cron())) }
                        span { (pr.currency.code()) " · " (pr.tz_name()) }
                    }
                    @if s.notification_channels.is_empty() {
                        p.muted {
                            (t(lang, "No notification channel yet. ", "Chưa có kênh thông báo. "))
                            a href="/admin/notifications" { (t(lang, "Set up Telegram/Discord", "Cài Telegram/Discord")) }
                        }
                    } @else {
                        ul.plain {
                            @for c in &s.notification_channels {
                                li.row {
                                    b { (c.channel) }
                                    span.badge class=[c.enabled.then_some("ok")] { (on_off(lang, c.enabled)) }
                                    code { (c.target_masked) }
                                    @match c.last_test_ok {
                                        Some(true) => span.badge.ok { (t(lang, "test OK ", "test OK ")) (fmt_local(c.last_test_at, short_datetime(lang), tz)) },
                                        Some(false) => span.badge.bad { (t(lang, "test failed ", "test lỗi ")) (fmt_local(c.last_test_at, short_datetime(lang), tz)) },
                                        None => span.muted { (t(lang, "not tested", "chưa test")) },
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        h2 { (t(lang, "Last crawl per supermarket", "Lần crawl gần nhất theo siêu thị")) }
        @if summaries.is_empty() {
            div.card { p.muted {
                (t(lang, "No supermarket yet. Type ", "Chưa có siêu thị. Gõ ")) code { "init" }
                (t(lang, " in your AI client to start, or ", " trong AI client để bắt đầu, hoặc "))
                a href="/admin/supermarkets" { (t(lang, "add a supermarket", "thêm siêu thị")) } "."
            } }
        }
        @for s in &summaries {
            div.card {
                h3 { a href={ "/admin/supermarkets/" (s.supermarket.id) } { (s.supermarket.name) } }
                div.row.muted {
                    span { (s.active_url_count) " URL" }
                    span { (s.pending_tasks) (t(lang, " tasks waiting", " task đang chờ")) }
                    @if s.needs_review_count > 0 { span.badge.warn { (s.needs_review_count) (t(lang, " to review", " cần duyệt")) } }
                }
                @match &s.last_run {
                    None => p.muted { (t(lang, "Never crawled.", "Chưa crawl lần nào.")) },
                    Some(r) => p {
                        span.badge class=[match r.status.as_str() { "success" => Some("ok"), "failed" => Some("bad"), "partial" => Some("warn"), _ => None }] { (r.status) }
                        " " (fmt_local(Some(r.started_at), long_datetime(lang), tz)) " (" (ago(Some(r.started_at), lang)) ") · "
                        (r.urls_ok) " ok, " (r.urls_failed) (t(lang, " failed", " lỗi"))
                    },
                }
            }
        }
        @if mcp {
            h2 { (t(lang, "Connect an AI client", "Kết nối AI client")) }
            div.card { p { (t(lang, "MCP connector URL: ", "URL connector MCP: ")) code { (st.app.config.mcp_url()) } } }
        }
    };
    Ok(page(t(lang, "Overview", "Tổng quan"), "/admin", &session, Some(&user), lang, body))
}
