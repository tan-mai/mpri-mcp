// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Path, State};
use maud::html;

use crate::clock::{ago, human};
use crate::services::schedules;
use crate::services::supermarkets::{self, Changes};
use crate::web::layout::{back, csrf_field, page};
use crate::web::session::Session;
use crate::web::{AppState, Page, check_csrf, require_user};

const LIST: &str = "/admin/supermarkets";

fn field<'a>(form: &'a HashMap<String, String>, key: &str) -> &'a str {
    form.get(key).map(String::as_str).unwrap_or_default()
}

pub async fn list(State(st): State<AppState>, Extension(session): Extension<Session>) -> Page {
    let user = require_user(&st, &session, LIST).await?;
    let summaries = supermarkets::list_summaries(&st.app.db).await.unwrap_or_default();
    let body = html! {
        h1 { "Siêu thị" }
        @for s in &summaries {
            @let m = &s.supermarket;
            div.card {
                h3 { a href={ (LIST) "/" (m.id) } { (m.name) } " " span.muted { "(" (m.id) ")" } }
                div.muted { a href=(m.base_url) rel="noreferrer" { (m.base_url) } }
                div.row.muted {
                    span { (s.active_url_count) " URL" }
                    span { "Lịch: " (m.cron_expression.as_deref().unwrap_or("—")) " · " (if m.cron_enabled { "bật" } else { "tắt" }) }
                    span { "Lần crawl cuối: " (s.last_run.as_ref().map(|r| ago(Some(r.started_at))).unwrap_or_else(|| "chưa có".into())) }
                }
                @if m.search_url_template.is_none() { p.muted { "Chưa có search_url_template." } }
            }
        }
        h2 { "Thêm siêu thị" }
        div.card {
            form.stack method="post" action=(LIST) {
                (csrf_field(&session))
                label { "Tên" input type="text" name="name" required placeholder="Bách Hóa Xanh"; }
                label { "Website (base_url)" input type="text" name="base_url" required placeholder="bachhoaxanh.com"; }
                label { "search_url_template (tùy chọn, chứa {q})" input type="text" name="search_url_template" placeholder="https://example.vn/tim-kiem?key={q}"; }
                div.actions { button type="submit" { "Thêm" } }
            }
        }
    };
    Ok(page("Siêu thị", LIST, &session, Some(&user), body))
}

pub async fn create(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, LIST).await?;
    let template = Some(field(&form, "search_url_template")).filter(|t| !t.trim().is_empty());
    Ok(match supermarkets::add(&st.app.db, field(&form, "name"), field(&form, "base_url"), template).await {
        Ok(m) => back(&session, "success", format!("Đã thêm {} (ID '{}').", m.name, m.id), &format!("{LIST}/{}", m.id)),
        Err(e) => back(&session, "error", e.to_string(), LIST),
    })
}

pub async fn detail(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<String>,
) -> Page {
    let here = format!("{LIST}/{id}");
    let user = require_user(&st, &session, &here).await?;
    let Ok(Some(m)) = supermarkets::get(&st.app.db, &id).await else {
        return Ok(back(&session, "error", supermarkets::not_found_message(&id), LIST));
    };
    let next = schedules::next_run(&m, st.app.server_cron()).map(|t| t.timestamp_millis());
    let body = html! {
        p { a href=(LIST) { "← Siêu thị" } }
        h1 { (m.name) }
        div.card {
            form.stack method="post" action=(here) {
                (csrf_field(&session))
                label { "Tên" input type="text" name="name" value=(m.name) required; }
                label { "Website (base_url)" input type="text" name="base_url" value=(m.base_url) required; }
                label { "search_url_template (để trống để xóa)" input type="text" name="search_url_template" value=(m.search_url_template.as_deref().unwrap_or("")); }
                div.actions { button type="submit" { "Lưu" } }
            }
        }
        h2 { "Lịch kiểm tra giá" }
        div.card {
            form.stack method="post" action={ (here) "/schedule" } {
                (csrf_field(&session))
                label { "Cron (5 trường, giờ Việt Nam)" input type="text" name="cron_expression" value=(m.cron_expression.as_deref().unwrap_or("")) placeholder="0 8 * * 6,0"; }
                label.check { input type="checkbox" name="enabled" checked[m.cron_enabled]; "Bật lịch" }
                div.actions { button type="submit" { "Lưu lịch" } }
            }
            @if !st.app.server_cron() {
                p.muted { (schedules::CRON_OFF_HINT) }
            } @else if let Some(n) = human(next) {
                p.muted { "Lần chạy tới: " (n) }
            }
        }
        h2 { "Xóa siêu thị" }
        div.card {
            p.muted { "Xóa siêu thị sẽ xóa luôn URL sản phẩm, lịch sử giá và task của siêu thị này." }
            form method="post" action={ (here) "/delete" } {
                (csrf_field(&session))
                label.check { input type="checkbox" name="confirm" required; "Tôi hiểu, xóa " (m.name) }
                div.actions { button.danger type="submit" { "Xóa" } }
            }
        }
    };
    Ok(page(&m.name, LIST, &session, Some(&user), body))
}

pub async fn update(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    let here = format!("{LIST}/{id}");
    require_user(&st, &session, &here).await?;
    let changes = Changes {
        name: Some(field(&form, "name").to_string()),
        base_url: Some(field(&form, "base_url").to_string()),
        search_url_template: Some(field(&form, "search_url_template").to_string()),
        timezone: None,
    };
    Ok(match supermarkets::update(&st.app.db, &id, changes).await {
        Ok(m) => back(&session, "success", format!("Đã lưu {}.", m.name), &here),
        Err(e) => back(&session, "error", e.to_string(), &here),
    })
}

pub async fn schedule(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    let here = format!("{LIST}/{id}");
    require_user(&st, &session, &here).await?;
    let expr = field(&form, "cron_expression").trim().to_string();
    let enabled = form.contains_key("enabled");
    let db = &st.app.db;
    let result = if expr.is_empty() {
        supermarkets::set_schedule_fields(db, &id, None, false).await.map(|_| "Đã xóa lịch.".to_string())
    } else {
        match schedules::set_cron(db, &id, &expr).await {
            Ok(r) if enabled => Ok(format!("Đã đặt lịch '{}'.", r.cron_expression)),
            Ok(r) => schedules::disable_cron(db, &id)
                .await
                .map(|_| format!("Đã lưu lịch '{}' (đang tắt).", r.cron_expression)),
            Err(e) => Err(e),
        }
    };
    Ok(match result {
        Ok(msg) => back(&session, "success", msg, &here),
        Err(e) => back(&session, "error", e.to_string(), &here),
    })
}

pub async fn delete(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, LIST).await?;
    Ok(match supermarkets::delete(&st.app.db, &id).await {
        Ok(()) => back(&session, "success", "Đã xóa siêu thị.", LIST),
        Err(e) => back(&session, "error", e.to_string(), LIST),
    })
}
