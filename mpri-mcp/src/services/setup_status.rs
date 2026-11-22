// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use crate::db::Db;
use crate::error::SResult;
use crate::services::{alerts, analytics, notify, products, supermarkets};

#[derive(Debug, Clone)]
pub struct MarketStatus {
    pub supermarket_id: String,
    pub name: String,
    pub has_search_url_template: bool,
    pub has_schedule: bool,
}

#[derive(Debug, Clone)]
pub struct ChannelStatus {
    pub channel: String,
    pub enabled: bool,
    pub target_masked: String,
    pub last_test_ok: Option<bool>,
    pub last_test_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct SetupStatus {
    pub supermarkets: Vec<MarketStatus>,
    pub products: i64,
    pub products_without_urls: i64,
    pub price_alerts: i64,
    pub notification_channels: Vec<ChannelStatus>,
    pub has_any_price: bool,
    pub next_step: &'static str,
}

pub async fn status(db: &Db) -> SResult<SetupStatus> {
    let markets: Vec<MarketStatus> = supermarkets::list(db)
        .await?
        .into_iter()
        .map(|m| MarketStatus {
            has_search_url_template: m.search_url_template.is_some(),
            has_schedule: m.cron_enabled && m.cron_expression.is_some(),
            supermarket_id: m.id,
            name: m.name,
        })
        .collect();
    let channels: Vec<ChannelStatus> = notify::list_channels(db)
        .await?
        .into_iter()
        .map(|c| ChannelStatus {
            target_masked: c.target_masked(),
            channel: c.channel,
            enabled: c.enabled,
            last_test_ok: c.last_test_ok,
            last_test_at: c.last_test_at,
        })
        .collect();
    let product_count = products::count(db).await?;
    let alert_count = alerts::count(db).await?;
    let has_any_price = analytics::has_any_price(db).await?;
    let has_schedule = markets.iter().any(|m| m.has_schedule);
    let has_channel = channels.iter().any(|c| c.enabled);
    let later_done = has_schedule || has_any_price;
    let next_step = if markets.is_empty() {
        "add_supermarket"
    } else if product_count == 0 {
        "add_product"
    } else if alert_count == 0 && !later_done {
        "set_alerts"
    } else if !has_channel && !later_done {
        "notifications"
    } else if !has_schedule {
        "schedule"
    } else if !has_any_price {
        "first_crawl"
    } else {
        "done"
    };
    Ok(SetupStatus {
        supermarkets: markets,
        products: product_count,
        products_without_urls: products::count_without_urls(db).await?,
        price_alerts: alert_count,
        notification_channels: channels,
        has_any_price,
        next_step,
    })
}
