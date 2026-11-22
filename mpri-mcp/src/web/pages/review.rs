// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use axum::Extension;
use axum::extract::{Form, Path, State};
use maud::html;

use crate::clock::fmt_local;
use crate::services::reviews;
use crate::text::vnd;
use crate::web::layout::{back, csrf_field, page};
use crate::web::session::Session;
use crate::web::{AppState, Page, check_csrf, require_user};

const LIST: &str = "/admin/review";

pub async fn list(State(st): State<AppState>, Extension(session): Extension<Session>) -> Page {
    let user = require_user(&st, &session, LIST).await?;
    let items = reviews::list(&st.app.db, 100).await.unwrap_or_default();
    let body = html! {
        h1 { "Giá cần duyệt" }
        p.muted { "Giá lệch hơn 50% so với lần trước. Accept nếu đúng, Reject để xóa giá sai." }
        @if items.is_empty() { div.card { p.muted { "Không có giá nào cần duyệt." } } }
        @for i in &items {
            div.card {
                h3 { a href={ "/admin/products/" (i.product_id) } { (i.product_name) } }
                div.row {
                    span { (i.supermarket_name) }
                    span.price { (vnd(Some(i.current_price))) }
                    span.muted { "trước đó " (vnd(i.previous_price)) }
                    span.muted { (fmt_local(Some(i.crawled_at), "%d/%m/%Y %H:%M")) }
                }
                @if let Some(t) = &i.raw_text { p.muted { "“" (t) "”" } }
                p { a href=(i.url) rel="noreferrer" target="_blank" { "Mở trang sản phẩm" } }
                div.actions {
                    form method="post" action={ "/admin/prices/" (i.record_id) "/accept" } {
                        (csrf_field(&session)) button type="submit" { "Accept" }
                    }
                    form method="post" action={ "/admin/prices/" (i.record_id) "/reject" } {
                        (csrf_field(&session)) button.danger type="submit" { "Reject" }
                    }
                }
            }
        }
    };
    Ok(page("Cần duyệt", LIST, &session, Some(&user), body))
}

pub async fn accept(
    State(st): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> Page {
    check_csrf(&session, &form)?;
    require_user(&st, &session, LIST).await?;
    Ok(match reviews::accept(&st.app.db, id).await {
        Ok(()) => back(&session, "success", "Đã chấp nhận giá.", LIST),
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
    Ok(match reviews::reject(&st.app.db, id).await {
        Ok(()) => back(&session, "success", "Đã xóa giá sai.", LIST),
        Err(e) => back(&session, "error", e.to_string(), LIST),
    })
}
