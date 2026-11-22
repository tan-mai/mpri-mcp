// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Path, Query, State};
use maud::html;

use crate::clock::fmt_local;
use crate::services::analytics::latest_prices;
use crate::services::products::{self, UrlInput};
use crate::services::supermarkets;
use crate::text::vnd;
use crate::web::layout::{back, csrf_field, page};
use crate::web::session::Session;
use crate::web::{AppState, Page, check_csrf, require_user};

const LIST: &str = "/admin/products";

pub async fn list(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<HashMap<String, String>>,
) -> Page {
    let user = require_user(&st, &session, LIST).await?;
    let query = q.get("q").cloned().unwrap_or_default();
    let items = products::list(&st.app.db, Some(&query)).await.unwrap_or_default();
    let mut rows = Vec::new();
    for item in items {
        let prices = latest_prices(&st.app.db, item.product.id).await.unwrap_or_default();
        rows.push((item, prices));
    }
    let body = html! {
        h1 { "Sản phẩm" }
        form.row method="get" action=(LIST) {
            input type="text" name="q" value=(query) placeholder="Tìm theo tên (không dấu cũng được)";
            button.secondary type="submit" { "Tìm" }
        }
        p.muted { "Thêm sản phẩm mới bằng AI (Workflow B), sau đó sửa ở đây." }
        @if rows.is_empty() { div.card { p.muted { "Chưa có sản phẩm nào." } } }
        @for (item, prices) in &rows {
            div.card {
                h3 { a href={ (LIST) "/" (item.product.id) } { (item.product.name) } }
                @if let Some(c) = &item.product.category { div.muted { (c) } }
                @if prices.is_empty() { p.muted { "Chưa có URL." } }
                ul.plain {
                    @for p in prices {
                        li.row {
                            span { (p.supermarket_name) }
                            span.price { (vnd(p.current_price)) }
                            @if let Some(d) = p.discount_pct.filter(|d| *d > 0.0) { span.badge.ok { "−" (d) "%" } }
                            @if p.needs_review { span.badge.warn { "cần duyệt" } }
                            @if p.in_stock == Some(false) { span.badge.bad { "hết hàng" } }
                            span.muted { (fmt_local(p.crawled_at, "%d/%m %H:%M")) }
                        }
                    }
                }
            }
        }
    };
    Ok(page("Sản phẩm", LIST, &session, Some(&user), body))
}

pub async fn detail(State(st): State<AppState>, Extension(session): Extension<Session>, Path(id): Path<i64>) -> Page {
    let here = format!("{LIST}/{id}");
    let user = require_user(&st, &session, &here).await?;
    let db = &st.app.db;
    let product = match products::get(db, id).await {
        Ok(p) => p,
        Err(e) => return Ok(back(&session, "error", e.to_string(), LIST)),
    };
    let urls = products::urls_of(db, id).await.unwrap_or_default();
    let prices = latest_prices(db, id).await.unwrap_or_default();
    let markets = supermarkets::list(db).await.unwrap_or_default();
    let body = html! {
        p { a href=(LIST) { "← Sản phẩm" } }
        h1 { (product.name) }
        div.card {
            form.stack method="post" action=(here) {
                (csrf_field(&session))
                label { "Tên" input type="text" name="name" value=(product.name) required; }
                label { "Danh mục" input type="text" name="category" value=(product.category.as_deref().unwrap_or("")); }
                div.actions { button type="submit" { "Lưu" } }
            }
        }
        h2 { "Giá mới nhất" }
        div.card {
            @if prices.is_empty() { p.muted { "Chưa có giá." } }
            ul.plain {
                @for p in &prices {
                    li.row {
                        span { (p.supermarket_name) }
                        span.price { (vnd(p.current_price)) }
                        @if p.original_price.is_some() && p.original_price != p.current_price { span.muted { "gốc " (vnd(p.original_price)) } }
                        @if let Some(m) = p.member_price { span.muted { "thành viên " (vnd(Some(m))) } }
                        @if p.needs_review { span.badge.warn { "cần duyệt" } }
                        span.muted { (fmt_local(p.crawled_at, "%d/%m/%Y %H:%M")) }
                    }
                }
            }
        }
        h2 { "URL theo siêu thị" }
        @for u in &urls {
            div.card {
                h3 { (u.supermarket_name) }
                form.stack method="post" action={ "/admin/urls/" (u.id) } {
                    (csrf_field(&session))
                    input type="hidden" name="product_id" value=(id);
                    label { "URL" input type="url" name="url" value=(u.url) required; }
                    label.check { input type="checkbox" name="is_active" checked[u.is_active]; "Đang theo dõi" }
                    @if let Some(e) = u.last_error.as_ref().filter(|_| u.consecutive_failures > 0) {
                        p.muted { "Lỗi gần nhất: " (e) }
                    }
                    div.actions { button type="submit" { "Lưu URL" } }
                }
                form method="post" action={ "/admin/urls/" (u.id) "/delete" } {
                    (csrf_field(&session))
                    input type="hidden" name="product_id" value=(id);
                    div.actions { button.danger type="submit" { "Xóa URL" } }
                }
            }
        }
        div.card {
            h3 { "Thêm URL" }
            form.stack method="post" action={ (here) "/urls" } {
                (csrf_field(&session))
                label { "Siêu thị"
                    select name="supermarket_id" required {
                        @for m in &markets { option value=(m.id) { (m.name) } }
                    }
                }
                label { "URL trang sản phẩm" input type="url" name="url" required; }
                div.actions { button type="submit" { "Thêm URL" } }
            }
        }
        h2 { "Xóa sản phẩm" }
        div.card {
            form method="post" action={ (here) "/delete" } {
                (csrf_field(&session))
                label.check { input type="checkbox" name="confirm" required; "Tôi hiểu, xóa sản phẩm, lịch sử giá và mốc giá" }
                div.actions { button.danger type="submit" { "Xóa sản phẩm" } }
            }
        }
    };
    Ok(page(&product.name, LIST, &session, Some(&user), body))
}

pub async fn update(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    let here = format!("{LIST}/{id}");
    require_user(&st, &session, &here).await?;
    let name = form.get("name").cloned().unwrap_or_default();
    Ok(match products::update(&st.app.db, id, &name, form.get("category").map(String::as_str)).await {
        Ok(p) => back(&session, "success", format!("Đã lưu {}.", p.name), &here),
        Err(e) => back(&session, "error", e.to_string(), &here),
    })
}

pub async fn add_url(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    let here = format!("{LIST}/{id}");
    require_user(&st, &session, &here).await?;
    let product = match products::get(&st.app.db, id).await {
        Ok(p) => p,
        Err(e) => return Ok(back(&session, "error", e.to_string(), LIST)),
    };
    let input = UrlInput {
        supermarket_id: form.get("supermarket_id").cloned().unwrap_or_default(),
        url: form.get("url").cloned().unwrap_or_default(),
    };
    Ok(match products::add_product_with_urls(&st.app.db, &product.name, None, &[input]).await {
        Ok(r) => {
            let u = &r.urls[0];
            match (&u.message, &u.warning) {
                (Some(err), _) => back(&session, "error", err.clone(), &here),
                (None, Some(w)) => back(&session, "warning", format!("URL {}: {w}", u.status), &here),
                (None, None) => back(&session, "success", format!("URL {}.", u.status), &here),
            }
        }
        Err(e) => back(&session, "error", e.to_string(), &here),
    })
}

fn product_page(form: &HashMap<String, String>) -> String {
    form.get("product_id")
        .and_then(|p| p.parse::<i64>().ok())
        .map(|p| format!("{LIST}/{p}"))
        .unwrap_or_else(|| LIST.to_string())
}

pub async fn update_url(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(url_id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    let target = product_page(&form);
    require_user(&st, &session, &target).await?;
    let url = form.get("url").map(String::as_str);
    let active = Some(form.contains_key("is_active"));
    Ok(match products::update_url(&st.app.db, url_id, url, active).await {
        Ok(_) => back(&session, "success", "Đã lưu URL.", &target),
        Err(e) => back(&session, "error", e.to_string(), &target),
    })
}

pub async fn delete_url(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(url_id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    let target = product_page(&form);
    require_user(&st, &session, &target).await?;
    Ok(match products::delete_url(&st.app.db, url_id).await {
        Ok(()) => back(&session, "success", "Đã xóa URL.", &target),
        Err(e) => back(&session, "error", e.to_string(), &target),
    })
}

pub async fn delete(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, LIST).await?;
    Ok(match products::delete(&st.app.db, id).await {
        Ok(()) => back(&session, "success", "Đã xóa sản phẩm.", LIST),
        Err(e) => back(&session, "error", e.to_string(), LIST),
    })
}
