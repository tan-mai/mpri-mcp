// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::collections::HashMap;

use crate::clock::{local, short_date, short_datetime};
use crate::db::Db;
use crate::error::SResult;
use crate::locale::{Lang, t, tf};
use crate::prefs::Prefs;
use crate::services::alerts::{self, AlertHit, AlertView};
use crate::services::analytics::{cheapest_by_product, discount_pct};
use crate::text::fmt_g;

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

fn discount(p: &Prefs, original: Option<i64>, price: i64) -> Vec<Seg> {
    match discount_pct(original, Some(price)) {
        Some(pct) if pct != 0.0 => {
            let was = p.currency.format_opt(original);
            let pct = fmt_g(pct);
            vec![s(tf(p.language, format!(" (was {was}, −{pct}%)"), format!(" (gốc {was}, −{pct}%)")))]
        }
        _ => Vec::new(),
    }
}

pub fn alert_lines(hit: &AlertHit, p: &Prefs) -> Vec<Line> {
    let lang = p.language;
    let money = |v: i64| p.currency.format(v);
    let target = money(hit.target_price);
    let mut lines = vec![vec![
        s(t(lang, "🔔 Price below target: ", "🔔 Giá dưới mốc: ")),
        b(&hit.product_name),
        s(tf(lang, format!(" (target {target})"), format!(" (mốc {target})"))),
    ]];
    for sp in &hit.prices {
        let at = tf(lang, format!(" at {}", sp.supermarket_name), format!(" tại {}", sp.supermarket_name));
        let mut line = vec![s("• "), b(money(sp.price)), s(at)];
        line.extend(discount(p, sp.original_price, sp.price));
        if let Some(m) = sp.member_price.filter(|m| *m != 0) {
            let member = money(m);
            line.push(s(tf(lang, format!(" · member {member}"), format!(" · thành viên {member}"))));
        }
        line.push(s(" · "));
        line.push(link(t(lang, "View product", "Xem sản phẩm"), &sp.url));
        if sp.needs_review {
            line.push(s(t(lang, " ⚠ unverified", " ⚠ chưa xác minh")));
        }
        lines.push(line);
    }
    if hit.prices.iter().any(|sp| sp.needs_review) {
        lines.push(vec![s(t(
            lang,
            "⚠ The price moved more than 50% since last time: check it before buying.",
            "⚠ Giá lệch hơn 50% so với lần trước: hãy kiểm tra lại trước khi mua.",
        ))]);
    }
    lines
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub fn test_lines(lang: Lang) -> Vec<Line> {
    vec![vec![b(t(lang, "✅ MPriMCP: notifications are working", "✅ MPriMCP: kết nối thông báo thành công"))]]
}

pub async fn summary_lines(db: &Db, since: Option<i64>, now: i64, p: &Prefs) -> SResult<Vec<Line>> {
    let (lang, tz) = (p.language, p.timezone);
    let money = |v: Option<i64>| p.currency.format_opt(v);
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
    let at = |name: &str| tf(lang, format!(" at {name}"), format!(" tại {name}"));

    let when = local(now, tz).format(short_datetime(lang)).to_string();
    let mut lines: Vec<Line> = vec![vec![b(tf(
        lang,
        format!("🛒 Cheapest prices — crawl {when}"),
        format!("🛒 Giá rẻ nhất — lượt quét {when}"),
    ))]];
    let plain_products: Vec<&(i64, String)> = products.iter().filter(|(id, _)| !alert_views.contains_key(id)).collect();
    for (id, name) in &plain_products {
        let Some(mp) = cheapest.get(id).filter(|m| m.current_price.is_some()) else {
            lines.push(vec![s(tf(lang, format!("• {name} — no price yet"), format!("• {name} — chưa có giá")))]);
            continue;
        };
        let price = mp.current_price.unwrap_or_default();
        let mut line =
            vec![s("• "), link(name.clone(), &mp.url), s(" — "), b(money(Some(price))), s(at(&mp.supermarket_name))];
        line.extend(discount(p, mp.original_price, price));
        if mp.needs_review {
            line.push(s(t(lang, " ⚠ unverified", " ⚠ chưa xác minh")));
        }
        if let (Some(since), Some(crawled)) = (since, mp.crawled_at)
            && crawled < since
        {
            let day = local(crawled, tz).format(short_date(lang)).to_string();
            line.push(s(tf(lang, format!(" · price from {day}"), format!(" · giá ngày {day}"))));
        }
        lines.push(line);
    }
    if plain_products.is_empty() && alert_views.is_empty() {
        lines.push(vec![s(t(lang, "No product is tracked yet.", "Chưa theo dõi sản phẩm nào."))]);
    }
    if !alert_views.is_empty() {
        lines.push(vec![b(t(lang, "🎯 Price targets", "🎯 Mốc giá"))]);
        let mut views: Vec<&AlertView> = alert_views.values().collect();
        views.sort_by(|a, b| a.product.name.cmp(&b.product.name));
        for v in views {
            let mut line = vec![s("• "), s(&v.product.name), s(format!(" < {}: ", money(Some(v.alert.target_price))))];
            match v.best.as_ref().filter(|best| best.current_price.is_some()) {
                None => line.push(s(t(lang, "no price yet", "chưa có giá"))),
                Some(best) if v.below() => line.extend([
                    s(t(lang, "✅ reached — ", "✅ đạt — ")),
                    b(money(best.current_price)),
                    s(at(&best.supermarket_name)),
                    s(" · "),
                    link(t(lang, "View", "Xem"), &best.url),
                ]),
                Some(best) => {
                    let cheapest = money(best.current_price);
                    line.extend([
                        s(tf(lang, format!("not yet — cheapest {cheapest}"), format!("chưa đạt — rẻ nhất {cheapest}"))),
                        s(at(&best.supermarket_name)),
                    ])
                }
            }
            lines.push(line);
        }
    }
    if !failed.is_empty() {
        lines.push(vec![b(t(lang, "⛔ Could not read this round", "⛔ Không đọc được lượt này"))]);
        for (product, market, error) in failed {
            let reason = error.unwrap_or_else(|| t(lang, "error", "lỗi").to_string());
            lines.push(vec![s(tf(
                lang,
                format!("• {product} at {market}: {reason}"),
                format!("• {product} tại {market}: {reason}"),
            ))]);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::Currency;
    use crate::services::alerts::SavedPrice;

    fn hit() -> AlertHit {
        AlertHit {
            product_id: 1,
            product_name: "Milk".into(),
            target_price: 400,
            prices: vec![SavedPrice {
                product_id: 1,
                product_name: "Milk".into(),
                supermarket_id: "s".into(),
                supermarket_name: "Store".into(),
                url: "https://x.example/p".into(),
                price: 399,
                original_price: Some(499),
                member_price: None,
                needs_review: false,
            }],
        }
    }

    #[test]
    fn alert_message_follows_language_and_currency() {
        let en = Prefs { currency: Currency::Usd, language: Lang::En, timezone: chrono_tz::America::New_York };
        let text = plain(&alert_lines(&hit(), &en));
        assert!(text.contains("Price below target: Milk (target $4.00)"), "{text}");
        assert!(text.contains("$3.99 at Store (was $4.99, −20%)"), "{text}");
        let vi = Prefs { currency: Currency::Vnd, language: Lang::Vi, timezone: chrono_tz::Asia::Ho_Chi_Minh };
        let text = plain(&alert_lines(&hit(), &vi));
        assert!(text.contains("Giá dưới mốc: Milk (mốc 400 ₫)"), "{text}");
        assert_eq!(plain(&test_lines(Lang::En)), "✅ MPriMCP: notifications are working");
    }
}
