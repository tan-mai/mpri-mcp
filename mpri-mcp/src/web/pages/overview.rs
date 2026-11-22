// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use axum::Extension;
use axum::extract::State;
use maud::html;

use crate::clock::{ago, fmt_local};
use crate::services::{agent_tasks, reviews, setup_status, supermarkets};
use crate::web::layout::page;
use crate::web::session::Session;
use crate::web::{AppState, Page, require_user};

fn step_label(step: &str) -> &'static str {
    match step {
        "add_supermarket" => "Thêm siêu thị",
        "add_product" => "Thêm sản phẩm",
        "set_alerts" => "Đặt mốc giá",
        "notifications" => "Cài kênh thông báo",
        "schedule" => "Đặt lịch kiểm tra giá",
        "first_crawl" => "Chạy crawl đầu tiên",
        _ => "Đã thiết lập xong",
    }
}

pub async fn overview(State(st): State<AppState>, Extension(session): Extension<Session>) -> Page {
    let user = require_user(&st, &session, "/admin").await?;
    let db = &st.app.db;
    let _ = agent_tasks::maintain(db).await;
    let status = setup_status::status(db).await.map_err(|e| e.to_string());
    let summaries = supermarkets::list_summaries(db).await.unwrap_or_default();
    let pending = agent_tasks::pending_total(db).await.unwrap_or(0);
    let review_count = reviews::count(db).await.unwrap_or(0);
    let mcp = st.app.mode == crate::config::Mode::Serve;
    let body = html! {
        h1 { "Tổng quan" }
        @match &status {
            Err(e) => div.flash.error { (e) },
            Ok(s) => {
                div.stats {
                    div.stat { b { (s.supermarkets.len()) } span.muted { "siêu thị" } }
                    div.stat { b { (s.products) } span.muted { "sản phẩm" } }
                    div.stat { b { (s.price_alerts) } span.muted { "mốc giá" } }
                    div.stat { b { (pending) } span.muted { "task chờ AI đọc giá" } }
                    div.stat { b { (review_count) } span.muted { "giá cần duyệt" } }
                }
                div.card {
                    h3 { "Bước tiếp theo: " (step_label(s.next_step)) }
                    div.row.muted {
                        span { "Sản phẩm chưa có URL: " (s.products_without_urls) }
                        span { "Đã có giá: " (if s.has_any_price { "có" } else { "chưa" }) }
                        span { "Cron trên máy chủ: " (if st.app.server_cron() { "bật" } else { "tắt" }) }
                    }
                    @if s.notification_channels.is_empty() {
                        p.muted { "Chưa có kênh thông báo. " a href="/admin/notifications" { "Cài Telegram/Discord" } }
                    } @else {
                        ul.plain {
                            @for c in &s.notification_channels {
                                li.row {
                                    b { (c.channel) }
                                    span.badge class=[c.enabled.then_some("ok")] { (if c.enabled { "bật" } else { "tắt" }) }
                                    code { (c.target_masked) }
                                    @match c.last_test_ok {
                                        Some(true) => span.badge.ok { "test OK " (fmt_local(c.last_test_at, "%d/%m %H:%M")) },
                                        Some(false) => span.badge.bad { "test lỗi " (fmt_local(c.last_test_at, "%d/%m %H:%M")) },
                                        None => span.muted { "chưa test" },
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        h2 { "Lần crawl gần nhất theo siêu thị" }
        @if summaries.is_empty() {
            div.card { p.muted { "Chưa có siêu thị. Gõ " code { "init" } " trong AI client để bắt đầu, hoặc " a href="/admin/supermarkets" { "thêm siêu thị" } "." } }
        }
        @for s in &summaries {
            div.card {
                h3 { a href={ "/admin/supermarkets/" (s.supermarket.id) } { (s.supermarket.name) } }
                div.row.muted {
                    span { (s.active_url_count) " URL" }
                    span { (s.pending_tasks) " task đang chờ" }
                    @if s.needs_review_count > 0 { span.badge.warn { (s.needs_review_count) " cần duyệt" } }
                }
                @match &s.last_run {
                    None => p.muted { "Chưa crawl lần nào." },
                    Some(r) => p {
                        span.badge class=[match r.status.as_str() { "success" => Some("ok"), "failed" => Some("bad"), "partial" => Some("warn"), _ => None }] { (r.status) }
                        " " (fmt_local(Some(r.started_at), "%d/%m/%Y %H:%M")) " (" (ago(Some(r.started_at))) ") · "
                        (r.urls_ok) " ok, " (r.urls_failed) " lỗi"
                    },
                }
            }
        }
        @if mcp {
            h2 { "Kết nối AI client" }
            div.card { p { "URL connector MCP: " code { (st.app.config.mcp_url()) } } }
        }
    };
    Ok(page("Tổng quan", "/admin", &session, Some(&user), body))
}
