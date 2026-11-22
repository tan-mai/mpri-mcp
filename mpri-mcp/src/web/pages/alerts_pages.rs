// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Path, State};
use maud::html;

use crate::services::{alerts, products};
use crate::text::vnd;
use crate::web::layout::{back, csrf_field, page};
use crate::web::pages::parse_vnd;
use crate::web::session::Session;
use crate::web::{AppState, Page, check_csrf, require_user};

const LIST: &str = "/admin/alerts";

pub async fn list(State(st): State<AppState>, Extension(session): Extension<Session>) -> Page {
    let user = require_user(&st, &session, LIST).await?;
    let views = alerts::list_alerts(&st.app.db).await.unwrap_or_default();
    let all_products = products::list(&st.app.db, None).await.unwrap_or_default();
    let body = html! {
        h1 { "Mốc giá (alert)" }
        p.muted { "Máy chủ nhắn Telegram/Discord ngay khi một lần crawl lưu giá thấp hơn mốc." }
        @if views.is_empty() { div.card { p.muted { "Chưa có mốc giá nào." } } }
        @for v in &views {
            div.card {
                h3 { a href={ "/admin/products/" (v.product.id) } { (v.product.name) } }
                div.row {
                    span { "Mốc: " b { (vnd(Some(v.alert.target_price))) } }
                    @match &v.best {
                        Some(b) => span { "Rẻ nhất: " (vnd(b.current_price)) " tại " (b.supermarket_name) },
                        None => span.muted { "Chưa có giá" },
                    }
                    @if v.below() { span.badge.ok { "đã dưới mốc" } }
                    @if let Some(ids) = &v.alert.supermarket_ids { span.muted { "Chỉ: " (ids.join(", ")) } }
                }
                form.row method="post" action={ (LIST) "/" (v.alert.id) } {
                    (csrf_field(&session))
                    input type="text" name="target_price" value=(v.alert.target_price) inputmode="numeric" required;
                    button.secondary type="submit" { "Sửa mốc" }
                }
                form method="post" action={ (LIST) "/" (v.alert.id) "/delete" } {
                    (csrf_field(&session))
                    div.actions { button.danger type="submit" { "Xóa" } }
                }
            }
        }
        h2 { "Thêm mốc giá" }
        div.card {
            @if all_products.is_empty() {
                p.muted { "Chưa có sản phẩm để đặt mốc." }
            } @else {
                form.stack method="post" action=(LIST) {
                    (csrf_field(&session))
                    label { "Sản phẩm"
                        select name="product_id" required {
                            @for p in &all_products { option value=(p.product.id) { (p.product.name) } }
                        }
                    }
                    label { "Báo khi giá dưới (VND)" input type="text" name="target_price" inputmode="numeric" placeholder="70.000" required; }
                    div.actions { button type="submit" { "Lưu" } }
                }
            }
        }
    };
    Ok(page("Alert", LIST, &session, Some(&user), body))
}

fn saved_message(v: &alerts::AlertView, created: bool) -> String {
    let mut msg = format!(
        "{} mốc: {} dưới {}.",
        if created { "Đã đặt" } else { "Đã sửa" },
        v.product.name,
        vnd(Some(v.alert.target_price))
    );
    if v.below() {
        msg.push_str(" Giá hiện tại đã dưới mốc.");
    }
    msg
}

pub async fn save(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, LIST).await?;
    let Some(target) = form.get("target_price").and_then(|t| parse_vnd(t)) else {
        return Ok(back(&session, "error", "Nhập giá bằng VND, ví dụ 70000.", LIST));
    };
    let Some(product_id) = form.get("product_id").and_then(|p| p.parse::<i64>().ok()) else {
        return Ok(back(&session, "error", "Chọn sản phẩm.", LIST));
    };
    Ok(match alerts::set_alert_for_product(&st.app.db, product_id, target, None).await {
        Ok((v, created)) => back(&session, "success", saved_message(&v, created), LIST),
        Err(e) => back(&session, "error", e.to_string(), LIST),
    })
}

pub async fn update(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, LIST).await?;
    let Some(target) = form.get("target_price").and_then(|t| parse_vnd(t)) else {
        return Ok(back(&session, "error", "Nhập giá bằng VND, ví dụ 70000.", LIST));
    };
    let views = alerts::list_alerts(&st.app.db).await.unwrap_or_default();
    let Some(current) = views.iter().find(|v| v.alert.id == id) else {
        return Ok(back(&session, "error", format!("Price alert #{id} not found."), LIST));
    };
    let scope = current.alert.supermarket_ids.clone();
    Ok(match alerts::set_alert_for_product(&st.app.db, current.product.id, target, scope.as_deref()).await {
        Ok((v, created)) => back(&session, "success", saved_message(&v, created), LIST),
        Err(e) => back(&session, "error", e.to_string(), LIST),
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
    Ok(match alerts::delete_alert_by_id(&st.app.db, id).await {
        Ok(()) => back(&session, "success", "Đã xóa mốc giá.", LIST),
        Err(e) => back(&session, "error", e.to_string(), LIST),
    })
}
