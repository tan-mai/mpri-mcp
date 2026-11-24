// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Path, Query, State};
use maud::html;

use crate::clock::{fmt_local, long_datetime, short_datetime};
use crate::locale::{t, tf};
use crate::prefs::load_or_default;
use crate::services::analytics::latest_prices;
use crate::services::products::{self, UrlInput};
use crate::services::supermarkets;
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
    let pr = load_or_default(&st.app.db).await;
    let (lang, tz, cur) = (pr.language, pr.timezone, pr.currency);
    let query = q.get("q").cloned().unwrap_or_default();
    let items = products::list(&st.app.db, Some(&query)).await.unwrap_or_default();
    let mut rows = Vec::new();
    for item in items {
        let prices = latest_prices(&st.app.db, item.product.id).await.unwrap_or_default();
        rows.push((item, prices));
    }
    let body = html! {
        h1 { (t(lang, "Products", "Sản phẩm")) }
        form.row method="get" action=(LIST) {
            input type="text" name="q" value=(query) placeholder=(t(lang, "Search by name (accents optional)", "Tìm theo tên (không dấu cũng được)"));
            button.secondary type="submit" { (t(lang, "Search", "Tìm")) }
        }
        p.muted { (t(lang, "Add new products with your AI (Workflow B), then edit them here.", "Thêm sản phẩm mới bằng AI (Workflow B), sau đó sửa ở đây.")) }
        @if rows.is_empty() { div.card { p.muted { (t(lang, "No product yet.", "Chưa có sản phẩm nào.")) } } }
        @for (item, prices) in &rows {
            div.card {
                h3 { a href={ (LIST) "/" (item.product.id) } { (item.product.name) } }
                @if let Some(c) = &item.product.category { div.muted { (c) } }
                @if prices.is_empty() { p.muted { (t(lang, "No URL yet.", "Chưa có URL.")) } }
                ul.plain {
                    @for p in prices {
                        li.row {
                            span { (p.supermarket_name) }
                            span.price { (cur.format_opt(p.current_price)) }
                            @if let Some(d) = p.discount_pct.filter(|d| *d > 0.0) { span.badge.ok { "−" (d) "%" } }
                            @if p.needs_review { span.badge.warn { (t(lang, "to review", "cần duyệt")) } }
                            @if p.in_stock == Some(false) { span.badge.bad { (t(lang, "out of stock", "hết hàng")) } }
                            span.muted { (fmt_local(p.crawled_at, short_datetime(lang), tz)) }
                        }
                    }
                }
            }
        }
    };
    Ok(page(t(lang, "Products", "Sản phẩm"), LIST, &session, Some(&user), lang, body))
}

pub async fn detail(State(st): State<AppState>, Extension(session): Extension<Session>, Path(id): Path<i64>) -> Page {
    let here = format!("{LIST}/{id}");
    let user = require_user(&st, &session, &here).await?;
    let db = &st.app.db;
    let pr = load_or_default(db).await;
    let (lang, tz, cur) = (pr.language, pr.timezone, pr.currency);
    let product = match products::get(db, id).await {
        Ok(p) => p,
        Err(e) => return Ok(back(&session, "error", e.to_string(), LIST)),
    };
    let urls = products::urls_of(db, id).await.unwrap_or_default();
    let prices = latest_prices(db, id).await.unwrap_or_default();
    let markets = supermarkets::list(db).await.unwrap_or_default();
    let body = html! {
        p { a href=(LIST) { (t(lang, "← Products", "← Sản phẩm")) } }
        h1 { (product.name) }
        div.card {
            form.stack method="post" action=(here) {
                (csrf_field(&session))
                label { (t(lang, "Name", "Tên")) input type="text" name="name" value=(product.name) required; }
                label { (t(lang, "Category", "Danh mục")) input type="text" name="category" value=(product.category.as_deref().unwrap_or("")); }
                div.actions { button type="submit" { (t(lang, "Save", "Lưu")) } }
            }
        }
        h2 { (t(lang, "Latest prices", "Giá mới nhất")) }
        div.card {
            @if prices.is_empty() { p.muted { (t(lang, "No price yet.", "Chưa có giá.")) } }
            ul.plain {
                @for p in &prices {
                    li.row {
                        span { (p.supermarket_name) }
                        span.price { (cur.format_opt(p.current_price)) }
                        @if p.original_price.is_some() && p.original_price != p.current_price {
                            span.muted { (t(lang, "was ", "gốc ")) (cur.format_opt(p.original_price)) }
                        }
                        @if let Some(m) = p.member_price { span.muted { (t(lang, "member ", "thành viên ")) (cur.format(m)) } }
                        @if p.needs_review { span.badge.warn { (t(lang, "to review", "cần duyệt")) } }
                        span.muted { (fmt_local(p.crawled_at, long_datetime(lang), tz)) }
                    }
                }
            }
        }
        h2 { (t(lang, "URLs per supermarket", "URL theo siêu thị")) }
        @for u in &urls {
            div.card {
                h3 { (u.supermarket_name) }
                form.stack method="post" action={ "/admin/urls/" (u.id) } {
                    (csrf_field(&session))
                    input type="hidden" name="product_id" value=(id);
                    label { "URL" input type="url" name="url" value=(u.url) required; }
                    label.check { input type="checkbox" name="is_active" checked[u.is_active]; (t(lang, "Tracked", "Đang theo dõi")) }
                    @if let Some(e) = u.last_error.as_ref().filter(|_| u.consecutive_failures > 0) {
                        p.muted { (t(lang, "Last error: ", "Lỗi gần nhất: ")) (e) }
                    }
                    div.actions { button type="submit" { (t(lang, "Save URL", "Lưu URL")) } }
                }
                form method="post" action={ "/admin/urls/" (u.id) "/delete" } {
                    (csrf_field(&session))
                    input type="hidden" name="product_id" value=(id);
                    div.actions { button.danger type="submit" { (t(lang, "Delete URL", "Xoá URL")) } }
                }
            }
        }
        div.card {
            h3 { (t(lang, "Add a URL", "Thêm URL")) }
            form.stack method="post" action={ (here) "/urls" } {
                (csrf_field(&session))
                label { (t(lang, "Supermarket", "Siêu thị"))
                    select name="supermarket_id" required {
                        @for m in &markets { option value=(m.id) { (m.name) } }
                    }
                }
                label { (t(lang, "Product page URL", "URL trang sản phẩm")) input type="url" name="url" required; }
                div.actions { button type="submit" { (t(lang, "Add URL", "Thêm URL")) } }
            }
        }
        h2 { (t(lang, "Delete product", "Xoá sản phẩm")) }
        div.card {
            form method="post" action={ (here) "/delete" } {
                (csrf_field(&session))
                label.check {
                    input type="checkbox" name="confirm" required;
                    (t(lang, "I understand, delete the product, its price history and its price alert",
                        "Tôi hiểu, xoá sản phẩm, lịch sử giá và mốc giá"))
                }
                div.actions { button.danger type="submit" { (t(lang, "Delete product", "Xoá sản phẩm")) } }
            }
        }
    };
    Ok(page(&product.name, LIST, &session, Some(&user), lang, body))
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
    let lang = load_or_default(&st.app.db).await.language;
    let name = form.get("name").cloned().unwrap_or_default();
    Ok(match products::update(&st.app.db, id, &name, form.get("category").map(String::as_str)).await {
        Ok(p) => {
            back(&session, "success", tf(lang, format!("Saved {}.", p.name), format!("Đã lưu {}.", p.name)), &here)
        }
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
    let lang = load_or_default(&st.app.db).await.language;
    let url = form.get("url").map(String::as_str);
    let active = Some(form.contains_key("is_active"));
    Ok(match products::update_url(&st.app.db, url_id, url, active).await {
        Ok(_) => back(&session, "success", t(lang, "URL saved.", "Đã lưu URL."), &target),
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
    let lang = load_or_default(&st.app.db).await.language;
    Ok(match products::delete_url(&st.app.db, url_id).await {
        Ok(()) => back(&session, "success", t(lang, "URL deleted.", "Đã xoá URL."), &target),
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
    let lang = load_or_default(&st.app.db).await.language;
    Ok(match products::delete(&st.app.db, id).await {
        Ok(()) => back(&session, "success", t(lang, "Product deleted.", "Đã xoá sản phẩm."), LIST),
        Err(e) => back(&session, "error", e.to_string(), LIST),
    })
}
