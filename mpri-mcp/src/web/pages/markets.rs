// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Path, State};
use maud::html;

use crate::clock::{ago, human};
use crate::locale::{t, tf};
use crate::prefs::load_or_default;
use crate::services::schedules;
use crate::services::supermarkets::{self, Changes};
use crate::web::layout::{back, csrf_field, on_off, page};
use crate::web::session::Session;
use crate::web::{AppState, Page, check_csrf, require_user};

const LIST: &str = "/admin/supermarkets";

fn field<'a>(form: &'a HashMap<String, String>, key: &str) -> &'a str {
    form.get(key).map(String::as_str).unwrap_or_default()
}

pub async fn list(State(st): State<AppState>, Extension(session): Extension<Session>) -> Page {
    let user = require_user(&st, &session, LIST).await?;
    let lang = load_or_default(&st.app.db).await.language;
    let summaries = supermarkets::list_summaries(&st.app.db).await.unwrap_or_default();
    let body = html! {
        h1 { (t(lang, "Supermarkets", "Siêu thị")) }
        @for s in &summaries {
            @let m = &s.supermarket;
            div.card {
                h3 { a href={ (LIST) "/" (m.id) } { (m.name) } " " span.muted { "(" (m.id) ")" } }
                div.muted { a href=(m.base_url) rel="noreferrer" { (m.base_url) } }
                div.row.muted {
                    span { (s.active_url_count) " URL" }
                    span { (t(lang, "Schedule: ", "Lịch: ")) (m.cron_expression.as_deref().unwrap_or("—")) " · " (on_off(lang, m.cron_enabled)) }
                    span {
                        (t(lang, "Last crawl: ", "Lần crawl cuối: "))
                        (s.last_run.as_ref().map(|r| ago(Some(r.started_at), lang)).unwrap_or_else(|| t(lang, "none", "chưa có").into()))
                    }
                }
                @if m.search_url_template.is_none() { p.muted { (t(lang, "No search_url_template yet.", "Chưa có search_url_template.")) } }
            }
        }
        h2 { (t(lang, "Add a supermarket", "Thêm siêu thị")) }
        div.card {
            form.stack method="post" action=(LIST) {
                (csrf_field(&session))
                label { (t(lang, "Name", "Tên")) input type="text" name="name" required; }
                label { (t(lang, "Website (base_url)", "Website (base_url)")) input type="text" name="base_url" required placeholder="example.com"; }
                label { (t(lang, "search_url_template (optional, contains {q})", "search_url_template (tuỳ chọn, chứa {q})"))
                    input type="text" name="search_url_template" placeholder="https://example.com/search?q={q}"; }
                div.actions { button type="submit" { (t(lang, "Add", "Thêm")) } }
            }
        }
    };
    Ok(page(t(lang, "Supermarkets", "Siêu thị"), LIST, &session, Some(&user), lang, body))
}

pub async fn create(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, LIST).await?;
    let lang = load_or_default(&st.app.db).await.language;
    let template = Some(field(&form, "search_url_template")).filter(|t| !t.trim().is_empty());
    Ok(match supermarkets::add(&st.app.db, field(&form, "name"), field(&form, "base_url"), template).await {
        Ok(m) => back(
            &session,
            "success",
            tf(lang, format!("Added {} (ID '{}').", m.name, m.id), format!("Đã thêm {} (ID '{}').", m.name, m.id)),
            &format!("{LIST}/{}", m.id),
        ),
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
    let pr = load_or_default(&st.app.db).await;
    let lang = pr.language;
    let Ok(Some(m)) = supermarkets::get(&st.app.db, &id).await else {
        return Ok(back(&session, "error", supermarkets::not_found_message(&id), LIST));
    };
    let next = schedules::next_run(&m, st.app.server_cron()).map(|t| t.timestamp_millis());
    let body = html! {
        p { a href=(LIST) { (t(lang, "← Supermarkets", "← Siêu thị")) } }
        h1 { (m.name) }
        div.card {
            form.stack method="post" action=(here) {
                (csrf_field(&session))
                label { (t(lang, "Name", "Tên")) input type="text" name="name" value=(m.name) required; }
                label { "Website (base_url)" input type="text" name="base_url" value=(m.base_url) required; }
                label { (t(lang, "search_url_template (empty removes it)", "search_url_template (để trống để xoá)"))
                    input type="text" name="search_url_template" value=(m.search_url_template.as_deref().unwrap_or("")); }
                div.actions { button type="submit" { (t(lang, "Save", "Lưu")) } }
            }
        }
        h2 { (t(lang, "Price check schedule", "Lịch kiểm tra giá")) }
        div.card {
            form.stack method="post" action={ (here) "/schedule" } {
                (csrf_field(&session))
                label { (tf(lang, format!("Cron (5 fields, {})", m.timezone), format!("Cron (5 trường, {})", m.timezone)))
                    input type="text" name="cron_expression" value=(m.cron_expression.as_deref().unwrap_or("")) placeholder="0 8 * * 6,0"; }
                label.check { input type="checkbox" name="enabled" checked[m.cron_enabled]; (t(lang, "Schedule on", "Bật lịch")) }
                div.actions { button type="submit" { (t(lang, "Save schedule", "Lưu lịch")) } }
            }
            @if !st.app.server_cron() {
                p.muted { (schedules::CRON_OFF_HINT) }
            } @else if let Some(n) = human(next, pr.timezone) {
                p.muted { (t(lang, "Next run: ", "Lần chạy tới: ")) (n) }
            }
        }
        h2 { (t(lang, "Delete supermarket", "Xoá siêu thị")) }
        div.card {
            p.muted { (t(lang,
                "Deleting the supermarket also deletes its product URLs, price history and tasks.",
                "Xoá siêu thị sẽ xoá luôn URL sản phẩm, lịch sử giá và task của siêu thị này.")) }
            form method="post" action={ (here) "/delete" } {
                (csrf_field(&session))
                label.check { input type="checkbox" name="confirm" required; (tf(lang, format!("I understand, delete {}", m.name), format!("Tôi hiểu, xoá {}", m.name))) }
                div.actions { button.danger type="submit" { (t(lang, "Delete", "Xoá")) } }
            }
        }
    };
    Ok(page(&m.name, LIST, &session, Some(&user), lang, body))
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
    let lang = load_or_default(&st.app.db).await.language;
    let changes = Changes {
        name: Some(field(&form, "name").to_string()),
        base_url: Some(field(&form, "base_url").to_string()),
        search_url_template: Some(field(&form, "search_url_template").to_string()),
        timezone: None,
    };
    Ok(match supermarkets::update(&st.app.db, &id, changes).await {
        Ok(m) => {
            back(&session, "success", tf(lang, format!("Saved {}.", m.name), format!("Đã lưu {}.", m.name)), &here)
        }
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
    let lang = load_or_default(&st.app.db).await.language;
    let expr = field(&form, "cron_expression").trim().to_string();
    let enabled = form.contains_key("enabled");
    let db = &st.app.db;
    let result = if expr.is_empty() {
        supermarkets::set_schedule_fields(db, &id, None, false)
            .await
            .map(|_| t(lang, "Schedule removed.", "Đã xoá lịch.").to_string())
    } else {
        match schedules::set_cron(db, &id, &expr).await {
            Ok(r) if enabled => Ok(tf(
                lang,
                format!("Schedule set to '{}'.", r.cron_expression),
                format!("Đã đặt lịch '{}'.", r.cron_expression),
            )),
            Ok(r) => schedules::disable_cron(db, &id).await.map(|_| {
                tf(
                    lang,
                    format!("Schedule '{}' saved (off).", r.cron_expression),
                    format!("Đã lưu lịch '{}' (đang tắt).", r.cron_expression),
                )
            }),
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
    let lang = load_or_default(&st.app.db).await.language;
    Ok(match supermarkets::delete(&st.app.db, &id).await {
        Ok(()) => back(&session, "success", t(lang, "Supermarket deleted.", "Đã xoá siêu thị."), LIST),
        Err(e) => back(&session, "error", e.to_string(), LIST),
    })
}
