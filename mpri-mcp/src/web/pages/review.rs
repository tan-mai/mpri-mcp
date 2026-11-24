// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Path, State};
use maud::html;

use crate::clock::{fmt_local, long_datetime};
use crate::locale::t;
use crate::prefs::load_or_default;
use crate::services::reviews;
use crate::web::layout::{back, csrf_field, page};
use crate::web::session::Session;
use crate::web::{AppState, Page, check_csrf, require_user};

const LIST: &str = "/admin/review";

pub async fn list(State(st): State<AppState>, Extension(session): Extension<Session>) -> Page {
    let user = require_user(&st, &session, LIST).await?;
    let pr = load_or_default(&st.app.db).await;
    let (lang, tz, cur) = (pr.language, pr.timezone, pr.currency);
    let items = reviews::list(&st.app.db, 100).await.unwrap_or_default();
    let body = html! {
        h1 { (t(lang, "Prices to review", "Giá cần duyệt")) }
        p.muted { (t(lang,
            "These prices moved more than 50% since the previous one. Accept a correct price, reject a wrong one to delete it.",
            "Giá lệch hơn 50% so với lần trước. Chấp nhận nếu đúng, loại bỏ để xoá giá sai.")) }
        @if items.is_empty() { div.card { p.muted { (t(lang, "Nothing to review.", "Không có giá nào cần duyệt.")) } } }
        @for i in &items {
            div.card {
                h3 { a href={ "/admin/products/" (i.product_id) } { (i.product_name) } }
                div.row {
                    span { (i.supermarket_name) }
                    span.price { (cur.format(i.current_price)) }
                    span.muted { (t(lang, "before ", "trước đó ")) (cur.format_opt(i.previous_price)) }
                    span.muted { (fmt_local(Some(i.crawled_at), long_datetime(lang), tz)) }
                }
                @if let Some(text) = &i.raw_text { p.muted { "“" (text) "”" } }
                p { a href=(i.url) rel="noreferrer" target="_blank" { (t(lang, "Open product page", "Mở trang sản phẩm")) } }
                div.actions {
                    form method="post" action={ "/admin/prices/" (i.record_id) "/accept" } {
                        (csrf_field(&session)) button type="submit" { (t(lang, "Accept", "Chấp nhận")) }
                    }
                    form method="post" action={ "/admin/prices/" (i.record_id) "/reject" } {
                        (csrf_field(&session)) button.danger type="submit" { (t(lang, "Reject", "Loại bỏ")) }
                    }
                }
            }
        }
    };
    Ok(page(t(lang, "Review", "Cần duyệt"), LIST, &session, Some(&user), lang, body))
}

pub async fn accept(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, LIST).await?;
    let lang = load_or_default(&st.app.db).await.language;
    Ok(match reviews::accept(&st.app.db, id).await {
        Ok(()) => back(&session, "success", t(lang, "Price accepted.", "Đã chấp nhận giá."), LIST),
        Err(e) => back(&session, "error", e.to_string(), LIST),
    })
}

pub async fn reject(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, LIST).await?;
    let lang = load_or_default(&st.app.db).await.language;
    Ok(match reviews::reject(&st.app.db, id).await {
        Ok(()) => back(&session, "success", t(lang, "Wrong price deleted.", "Đã xoá giá sai."), LIST),
        Err(e) => back(&session, "error", e.to_string(), LIST),
    })
}
