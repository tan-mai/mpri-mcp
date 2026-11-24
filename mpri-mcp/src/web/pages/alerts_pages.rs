// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Path, State};
use maud::html;

use crate::locale::{Lang, t, tf};
use crate::prefs::{Prefs, load_or_default};
use crate::services::{alerts, products};
use crate::web::layout::{back, csrf_field, page};
use crate::web::session::Session;
use crate::web::{AppState, Page, check_csrf, require_user};

const LIST: &str = "/admin/alerts";

pub async fn list(State(st): State<AppState>, Extension(session): Extension<Session>) -> Page {
    let user = require_user(&st, &session, LIST).await?;
    let pr = load_or_default(&st.app.db).await;
    let (lang, cur) = (pr.language, pr.currency);
    let views = alerts::list_alerts(&st.app.db).await.unwrap_or_default();
    let all_products = products::list(&st.app.db, None).await.unwrap_or_default();
    let body = html! {
        h1 { (t(lang, "Price alerts", "Mốc giá")) }
        p.muted { (t(lang,
            "The server messages you on Telegram/Discord as soon as a crawl saves a price below the target.",
            "Máy chủ nhắn Telegram/Discord ngay khi một lần crawl lưu giá thấp hơn mốc.")) }
        @if views.is_empty() { div.card { p.muted { (t(lang, "No price alert yet.", "Chưa có mốc giá nào.")) } } }
        @for v in &views {
            div.card {
                h3 { a href={ "/admin/products/" (v.product.id) } { (v.product.name) } }
                div.row {
                    span { (t(lang, "Target: ", "Mốc: ")) b { (cur.format(v.alert.target_price)) } }
                    @match &v.best {
                        Some(b) => span { (t(lang, "Cheapest: ", "Rẻ nhất: ")) (cur.format_opt(b.current_price)) (t(lang, " at ", " tại ")) (b.supermarket_name) },
                        None => span.muted { (t(lang, "No price yet", "Chưa có giá")) },
                    }
                    @if v.below() { span.badge.ok { (t(lang, "below target", "đã dưới mốc")) } }
                    @if let Some(ids) = &v.alert.supermarket_ids { span.muted { (t(lang, "Only: ", "Chỉ: ")) (ids.join(", ")) } }
                }
                form.row method="post" action={ (LIST) "/" (v.alert.id) } {
                    (csrf_field(&session))
                    input type="text" name="target_price" value=(cur.format(v.alert.target_price)) inputmode="decimal" required;
                    button.secondary type="submit" { (t(lang, "Change target", "Sửa mốc")) }
                }
                form method="post" action={ (LIST) "/" (v.alert.id) "/delete" } {
                    (csrf_field(&session))
                    div.actions { button.danger type="submit" { (t(lang, "Delete", "Xoá")) } }
                }
            }
        }
        h2 { (t(lang, "Add a price alert", "Thêm mốc giá")) }
        div.card {
            @if all_products.is_empty() {
                p.muted { (t(lang, "No product to set an alert on yet.", "Chưa có sản phẩm để đặt mốc.")) }
            } @else {
                form.stack method="post" action=(LIST) {
                    (csrf_field(&session))
                    label { (t(lang, "Product", "Sản phẩm"))
                        select name="product_id" required {
                            @for p in &all_products { option value=(p.product.id) { (p.product.name) } }
                        }
                    }
                    label { (tf(lang, format!("Notify below ({})", cur.code()), format!("Báo khi giá dưới ({})", cur.code())))
                        input type="text" name="target_price" inputmode="decimal" placeholder=(cur.example()) required; }
                    div.actions { button type="submit" { (t(lang, "Save", "Lưu")) } }
                }
            }
        }
    };
    Ok(page(t(lang, "Price alerts", "Mốc giá"), LIST, &session, Some(&user), lang, body))
}

fn saved_message(v: &alerts::AlertView, created: bool, pr: &Prefs) -> String {
    let (name, target) = (&v.product.name, pr.currency.format(v.alert.target_price));
    let mut msg = match (pr.language, created) {
        (Lang::En, true) => format!("Alert set: {name} below {target}."),
        (Lang::En, false) => format!("Alert changed: {name} below {target}."),
        (Lang::Vi, true) => format!("Đã đặt mốc: {name} dưới {target}."),
        (Lang::Vi, false) => format!("Đã sửa mốc: {name} dưới {target}."),
    };
    if v.below() {
        msg.push_str(t(pr.language, " The price is already below it.", " Giá hiện tại đã dưới mốc."));
    }
    msg
}

fn bad_price(pr: &Prefs) -> String {
    tf(
        pr.language,
        format!("Enter the price in {}, e.g. {}.", pr.currency.code(), pr.currency.example()),
        format!("Nhập giá bằng {}, ví dụ {}.", pr.currency.code(), pr.currency.example()),
    )
}

pub async fn save(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, LIST).await?;
    let pr = load_or_default(&st.app.db).await;
    let Some(target) = form.get("target_price").and_then(|v| pr.currency.parse_input(v)) else {
        return Ok(back(&session, "error", bad_price(&pr), LIST));
    };
    let Some(product_id) = form.get("product_id").and_then(|p| p.parse::<i64>().ok()) else {
        return Ok(back(&session, "error", t(pr.language, "Choose a product.", "Chọn sản phẩm."), LIST));
    };
    Ok(match alerts::set_alert_for_product(&st.app.db, product_id, target, None).await {
        Ok((v, created)) => back(&session, "success", saved_message(&v, created, &pr), LIST),
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
    let pr = load_or_default(&st.app.db).await;
    let Some(target) = form.get("target_price").and_then(|v| pr.currency.parse_input(v)) else {
        return Ok(back(&session, "error", bad_price(&pr), LIST));
    };
    let views = alerts::list_alerts(&st.app.db).await.unwrap_or_default();
    let Some(current) = views.iter().find(|v| v.alert.id == id) else {
        return Ok(back(&session, "error", format!("Price alert #{id} not found."), LIST));
    };
    let scope = current.alert.supermarket_ids.clone();
    Ok(match alerts::set_alert_for_product(&st.app.db, current.product.id, target, scope.as_deref()).await {
        Ok((v, created)) => back(&session, "success", saved_message(&v, created, &pr), LIST),
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
    let lang = load_or_default(&st.app.db).await.language;
    Ok(match alerts::delete_alert_by_id(&st.app.db, id).await {
        Ok(()) => back(&session, "success", t(lang, "Price alert deleted.", "Đã xoá mốc giá."), LIST),
        Err(e) => back(&session, "error", e.to_string(), LIST),
    })
}
