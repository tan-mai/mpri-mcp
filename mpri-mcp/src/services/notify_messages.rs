// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use crate::clock::local;
use crate::db::Db;
use crate::error::SResult;
use crate::services::alerts::{self, AlertHit, AlertView};
use crate::services::analytics::{cheapest_by_product, discount_pct};
use crate::text::{fmt_g, vnd};

#[derive(Debug, Clone)]
pub struct Seg {
    pub text: String,
    pub bold: bool,
    pub url: Option<String>,
}

pub type Line = Vec<Seg>;

fn s(text: impl Into<String>) -> Seg {
    Seg { text: text.into(), bold: false, url: None }
}

fn b(text: impl Into<String>) -> Seg {
    Seg { text: text.into(), bold: true, url: None }
}

fn link(text: impl Into<String>, url: &str) -> Seg {
    Seg { text: text.into(), bold: false, url: Some(url.to_string()) }
}

pub fn plain(lines: &[Line]) -> String {
    lines.iter().map(|l| l.iter().map(|seg| seg.text.as_str()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

fn discount(original: Option<i64>, price: i64) -> Vec<Seg> {
    match discount_pct(original, Some(price)) {
        Some(pct) if pct != 0.0 => vec![s(format!(" (gốc {}, −{}%)", vnd(original), fmt_g(pct)))],
        _ => Vec::new(),
    }
}

pub fn alert_lines(hit: &AlertHit) -> Vec<Line> {
    let mut lines =
        vec![vec![s("🔔 Giá dưới mốc: "), b(&hit.product_name), s(format!(" (mốc {})", vnd(Some(hit.target_price))))]];
    for p in &hit.prices {
        let mut line = vec![s("• "), b(vnd(Some(p.price))), s(format!(" tại {}", p.supermarket_name))];
        line.extend(discount(p.original_price, p.price));
        if let Some(m) = p.member_price.filter(|m| *m != 0) {
            line.push(s(format!(" · thành viên {}", vnd(Some(m)))));
        }
        line.push(s(" · "));
        line.push(link("Xem sản phẩm", &p.url));
        if p.needs_review {
            line.push(s(" ⚠ chưa xác minh"));
        }
        lines.push(line);
    }
    if hit.prices.iter().any(|p| p.needs_review) {
        lines.push(vec![s("⚠ Giá lệch hơn 50% so với lần trước: hãy kiểm tra lại trước khi mua.")]);
    }
    lines
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub fn test_lines() -> Vec<Line> {
    vec![vec![b("✅ MPriMCP: kết nối thông báo thành công")]]
}

pub async fn summary_lines(db: &Db, since: Option<i64>, now: i64) -> SResult<Vec<Line>> {
    let products: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, name FROM products WHERE id IN (SELECT product_id FROM product_urls WHERE is_active = 1) \
         ORDER BY name",
    )
    .fetch_all(db)
    .await?;
    let failed: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT p.name, s.name, t.last_error FROM crawl_tasks t \
         JOIN product_urls u ON u.id = t.product_url_id JOIN products p ON p.id = u.product_id \
         JOIN supermarkets s ON s.id = u.supermarket_id \
         WHERE t.status = 'failed' AND (?1 IS NULL OR t.completed_at >= ?1) ORDER BY t.completed_at",
    )
    .bind(since)
    .fetch_all(db)
    .await?;
    let cheapest = cheapest_by_product(db).await?;
    let alert_views: HashMap<i64, AlertView> =
        alerts::list_alerts(db).await?.into_iter().map(|v| (v.product.id, v)).collect();

    let mut lines: Vec<Line> =
        vec![vec![b(format!("🛒 Giá rẻ nhất — lượt quét {}", local(now).format("%d/%m %H:%M")))]];
    let plain_products: Vec<&(i64, String)> = products.iter().filter(|(id, _)| !alert_views.contains_key(id)).collect();
    for (id, name) in &plain_products {
        let Some(mp) = cheapest.get(id).filter(|m| m.current_price.is_some()) else {
            lines.push(vec![s(format!("• {name} — chưa có giá"))]);
            continue;
        };
        let price = mp.current_price.unwrap_or_default();
        let mut line = vec![
            s("• "),
            link(name.clone(), &mp.url),
            s(" — "),
            b(vnd(Some(price))),
            s(format!(" tại {}", mp.supermarket_name)),
        ];
        line.extend(discount(mp.original_price, price));
        if mp.needs_review {
            line.push(s(" ⚠ chưa xác minh"));
        }
        if let (Some(since), Some(at)) = (since, mp.crawled_at)
            && at < since
        {
            line.push(s(format!(" · giá ngày {}", local(at).format("%d/%m"))));
        }
        lines.push(line);
    }
    if plain_products.is_empty() && alert_views.is_empty() {
        lines.push(vec![s("Chưa theo dõi sản phẩm nào.")]);
    }
    if !alert_views.is_empty() {
        lines.push(vec![b("🎯 Mốc giá")]);
        let mut views: Vec<&AlertView> = alert_views.values().collect();
        views.sort_by(|a, b| a.product.name.cmp(&b.product.name));
        for v in views {
            let mut line = vec![s("• "), s(&v.product.name), s(format!(" < {}: ", vnd(Some(v.alert.target_price))))];
            match v.best.as_ref().filter(|best| best.current_price.is_some()) {
                None => line.push(s("chưa có giá")),
                Some(best) if v.below() => line.extend([
                    s("✅ đạt — "),
                    b(vnd(best.current_price)),
                    s(format!(" tại {}", best.supermarket_name)),
                    s(" · "),
                    link("Xem", &best.url),
                ]),
                Some(best) => line.extend([
                    s(format!("chưa đạt — rẻ nhất {}", vnd(best.current_price))),
                    s(format!(" tại {}", best.supermarket_name)),
                ]),
            }
            lines.push(line);
        }
    }
    if !failed.is_empty() {
        lines.push(vec![b("⛔ Không đọc được lượt này")]);
        for (product, market, error) in failed {
            lines.push(vec![s(format!("• {product} tại {market}: {}", error.as_deref().unwrap_or("lỗi")))]);
        }
    }
    Ok(lines)
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn attr_escape(text: &str) -> String {
    html_escape(text).replace('"', "&quot;").replace('\'', "&#x27;")
}

pub fn render_telegram(line: &Line) -> String {
    line.iter()
        .map(|seg| {
            let mut text = html_escape(&seg.text);
            if let Some(url) = &seg.url {
                text = format!("<a href=\"{}\">{text}</a>", attr_escape(url));
            }
            if seg.bold {
                text = format!("<b>{text}</b>");
            }
            text
        })
        .collect()
}

pub fn render_discord(line: &Line) -> String {
    line.iter()
        .map(|seg| {
            let mut text = String::new();
            for c in seg.text.chars() {
                if matches!(c, '\\' | '*' | '_' | '~' | '`' | '|' | '>' | '[' | ']') {
                    text.push('\\');
                }
                text.push(c);
            }
            if let Some(url) = &seg.url {
                text = format!("[{text}](<{url}>)");
            }
            if seg.bold {
                text = format!("**{text}**");
            }
            text
        })
        .collect()
}

pub fn split_messages(lines: &[Line], render: fn(&Line) -> String, limit: usize) -> Vec<String> {
    let mut messages = Vec::new();
    let mut current = String::new();
    for line in lines {
        let mut text = render(line);
        if text.chars().count() > limit {
            text = text.chars().take(limit - 1).collect::<String>() + "…";
        }
        let candidate = if current.is_empty() { text.clone() } else { format!("{current}\n{text}") };
        if candidate.chars().count() > limit {
            messages.push(std::mem::take(&mut current));
            current = text;
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        messages.push(current);
    }
    messages
}
