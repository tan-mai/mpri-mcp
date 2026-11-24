// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::locale::Money;
use crate::mcp::de;

fn default_limit() -> i64 {
    20
}
fn default_days_7() -> i64 {
    7
}
fn default_days_30() -> i64 {
    30
}
fn default_limit_50() -> i64 {
    50
}

// ------------------------------------------------------------------ inputs

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddSupermarketParams {
    /// Display name, e.g. 'Lotte Mart'.
    pub name: String,
    /// Website domain or URL, e.g. 'lottemart.vn'.
    pub base_url: String,
    #[serde(default)]
    /// Optional. Search-results URL of the supermarket's website with {q} where the keyword goes, e.g.
    /// 'https://cooponline.vn/search?router=productListing&query={q}'. Copy it from the browser after searching on
    /// the site and replace the keyword with {q}.
    pub search_url_template: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateSupermarketParams {
    /// Supermarket ID (or name) from get_supermarkets.
    pub supermarket_id: String,
    /// New display name.
    #[serde(default)]
    pub name: Option<String>,
    /// New website domain or URL.
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    /// Search-results URL of the supermarket's website with {q} where the keyword goes, e.g.
    /// 'https://cooponline.vn/search?router=productListing&query={q}'. Copy it from the browser after searching on
    /// the site and replace the keyword with {q}. Pass an empty string to remove it.
    pub search_url_template: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchUrlsParams {
    /// What to search for, e.g. 'sữa TH ít đường 110ml'.
    pub keyword: String,
    /// Supermarket IDs (or names) from get_supermarkets; default: all.
    #[serde(default, deserialize_with = "de::opt_list")]
    pub supermarket_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SupermarketParams {
    /// Supermarket ID (or name) from get_supermarkets.
    pub supermarket_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronParams {
    /// Supermarket ID (or name) from get_supermarkets.
    pub supermarket_id: String,
    /// Standard 5-field cron (minute hour day month weekday) in the server's timezone (get_setup_status.timezone). '0 3 * * *' = 03:00
    /// daily; '30 6 * * 1-5' = 06:30 Mon–Fri; weekday 0 or 7 = Sunday. Macros like @daily also work.
    pub cron_expression: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ProductUrlInput {
    /// ID (or name) from get_supermarkets, e.g. 'winmart'.
    pub supermarket_id: String,
    /// Full product page URL at that supermarket.
    pub url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddProductParams {
    /// Product name, e.g. 'Thùng Sữa TH True Milk Ít Đường 110ml'.
    pub name: String,
    /// The product's page at each supermarket (one URL per supermarket).
    #[serde(deserialize_with = "de::list")]
    pub urls: Vec<ProductUrlInput>,
    /// Optional category, e.g. 'Sữa'.
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProductNameParams {
    /// Product name; fuzzy and accent-insensitive ('sua th it duong').
    pub product_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeepDiscountParams {
    /// Minimum discount vs. the original price, in percent (1–99).
    #[serde(deserialize_with = "de::req_i64")]
    #[schemars(with = "i64")]
    pub min_discount_percentage: i64,
    /// Only prices checked within this many days.
    #[serde(default = "default_days_7", deserialize_with = "de::req_i64")]
    #[schemars(with = "i64")]
    pub days: i64,
    /// Maximum number of items.
    #[serde(default = "default_limit_50", deserialize_with = "de::req_i64")]
    #[schemars(with = "i64")]
    pub limit: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HistoryParams {
    /// Product name; fuzzy and accent-insensitive.
    pub product_name: String,
    /// How many days back (1–730).
    #[serde(default = "default_days_30", deserialize_with = "de::req_i64")]
    #[schemars(with = "i64")]
    pub days: i64,
    /// Limit to one supermarket (optional).
    #[serde(default)]
    pub supermarket_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClaimParams {
    /// Only this supermarket (optional).
    #[serde(default)]
    pub supermarket_id: Option<String>,
    /// How many tasks to take (1-50).
    #[serde(default = "default_limit", deserialize_with = "de::req_i64")]
    #[schemars(with = "i64")]
    pub limit: i64,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Ok,
    OutOfStock,
    NotFound,
    Error,
}

impl ResultStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ResultStatus::Ok => "ok",
            ResultStatus::OutOfStock => "out_of_stock",
            ResultStatus::NotFound => "not_found",
            ResultStatus::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PriceResultInput {
    /// task_id from get_pending_crawl_tasks.
    pub task_id: String,
    /// ok = price read; out_of_stock = page says sold out / 'hết hàng'; not_found = page is gone (404) or shows a
    /// different product; error = the page did not load or no price could be read (the task is retried later).
    pub status: ResultStatus,
    /// Required when status is ok: what a regular customer pays now for this exact item, as a number in the
    /// server's currency (get_setup_status.currency): VND has no decimals ('45.900đ' -> 45900), USD has cents
    /// ('$3.99' -> 3.99).
    #[serde(default, deserialize_with = "de::opt_num")]
    #[schemars(with = "Option<f64>")]
    pub price: Option<f64>,
    /// Struck-through / pre-discount price, if shown. Must be >= price.
    #[serde(default, deserialize_with = "de::opt_num")]
    #[schemars(with = "Option<f64>")]
    pub original_price: Option<f64>,
    /// Separate member/loyalty-card price, if shown.
    #[serde(default, deserialize_with = "de::opt_num")]
    #[schemars(with = "Option<f64>")]
    pub member_price: Option<f64>,
    /// Short page text the price was read from (max 500 characters).
    #[serde(default)]
    pub raw_text: Option<String>,
    /// Reason for an error, or a remark (max 300).
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubmitParams {
    /// One result per claimed task, at most 50 per call.
    #[serde(deserialize_with = "de::list")]
    pub results: Vec<PriceResultInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetAlertParams {
    /// Tracked product; fuzzy, accent-insensitive.
    pub product_name: String,
    /// Notify when the price is BELOW this, as a number in the server's currency (e.g. 70000 VND or 3.50 USD).
    #[serde(deserialize_with = "de::req_num")]
    #[schemars(with = "f64")]
    pub target_price: f64,
    /// Only these supermarkets (IDs or names from get_supermarkets); default all.
    #[serde(default, deserialize_with = "de::opt_list")]
    pub supermarket_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteAlertParams {
    /// Product whose alert to remove (fuzzy).
    pub product_name: String,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChannelName {
    Telegram,
    Discord,
}

impl ChannelName {
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelName::Telegram => "telegram",
            ChannelName::Discord => "discord",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetChannelParams {
    /// telegram or discord.
    pub channel: ChannelName,
    /// Telegram bot token from @BotFather ('123456789:AA…'), or the Discord channel webhook URL. An empty string
    /// removes this channel.
    pub token: String,
    /// Telegram only (required): the chat ID that receives the messages, e.g. 123456789, -100… or @channelname.
    #[serde(default)]
    pub chat_id: Option<String>,
    /// Send messages on this channel (default true).
    #[serde(default = "default_true_opt", deserialize_with = "de::opt_bool")]
    #[schemars(with = "Option<bool>")]
    pub enabled: Option<bool>,
}

fn default_true_opt() -> Option<bool> {
    Some(true)
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
pub enum CurrencyCode {
    #[serde(alias = "vnd")]
    VND,
    #[serde(alias = "usd")]
    USD,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LanguageCode {
    En,
    Vi,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetPreferencesParams {
    /// Currency of every price: VND or USD. Can only change while no price is stored.
    #[serde(default)]
    pub currency: Option<CurrencyCode>,
    /// Language of the dashboard and of the Telegram/Discord messages: en or vi.
    #[serde(default)]
    pub language: Option<LanguageCode>,
    /// IANA timezone for schedules and displayed times, e.g. Asia/Ho_Chi_Minh or America/New_York.
    #[serde(default)]
    pub timezone: Option<String>,
}

// ------------------------------------------------------------------ outputs

#[derive(Debug, Serialize, JsonSchema)]
pub struct ScheduleOut {
    pub cron_expression: Option<String>,
    pub enabled: bool,
    pub timezone: String,
    pub next_run: Option<String>,
    pub crawl_in_progress: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LastRunOut {
    pub status: String,
    pub trigger: String,
    pub started_at: Option<String>,
    pub urls_ok: i64,
    pub urls_failed: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SupermarketOut {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub schedule: ScheduleOut,
    pub tracked_urls: i64,
    pub failing_urls: i64,
    pub search_url_template: Option<String>,
    pub pending_tasks: i64,
    pub needs_review_count: i64,
    pub last_run: Option<LastRunOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SupermarketsOut {
    pub supermarkets: Vec<SupermarketOut>,
    pub timezone: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AddSupermarketOut {
    pub id: String,
    pub supermarket_id: String,
    pub name: String,
    pub base_url: String,
    pub search_url_template: Option<String>,
    pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct UpdateSupermarketOut {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub search_url_template: Option<String>,
    pub updated: Vec<String>,
    pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchUrlOut {
    pub supermarket_id: String,
    pub name: String,
    pub base_url: String,
    pub search_url: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchUrlsOut {
    pub keyword: String,
    pub supermarkets: Vec<SearchUrlOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CronOut {
    pub supermarket_id: String,
    pub supermarket_name: String,
    pub cron_expression: String,
    pub timezone: String,
    pub next_runs: Vec<String>,
    pub message: String,
    pub cron_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DisableCronOut {
    pub supermarket_id: String,
    pub cron_expression: Option<String>,
    pub enabled: bool,
    pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TriggerOut {
    pub supermarket_id: String,
    pub started: bool,
    pub queued_tasks: i64,
    pub already_open: i64,
    pub pending_tasks: i64,
    pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct UrlResultOut {
    pub supermarket_id: String,
    pub url: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AddProductOut {
    pub product_id: i64,
    pub name: String,
    pub category: Option<String>,
    pub created: bool,
    pub urls: Vec<UrlResultOut>,
    pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProductRef {
    pub id: i64,
    pub name: String,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PriceRowOut {
    pub supermarket_id: String,
    pub supermarket_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discount_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_discount_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_stock: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_data_yet: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_review: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ComparisonOut {
    pub product: ProductRef,
    pub currency: String,
    pub prices: Vec<PriceRowOut>,
    pub cheapest: Option<PriceRowOut>,
    pub other_matches: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DiscountOut {
    pub product_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub supermarket_id: String,
    pub supermarket_name: String,
    pub current_price: Money,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_price: Option<Money>,
    pub discount_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_discount_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
    pub url: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DiscountsOut {
    pub currency: String,
    pub count: usize,
    pub items: Vec<DiscountOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HistoryPointOut {
    pub date: String,
    pub current_price: Money,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_review: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HistoryStatsOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_vs_min_pct: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SeriesOut {
    pub supermarket_id: String,
    pub supermarket_name: String,
    pub points: Vec<HistoryPointOut>,
    pub stats: HistoryStatsOut,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HistoryProductRef {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HistoryOut {
    pub product: HistoryProductRef,
    pub days: i64,
    pub currency: String,
    pub series: Vec<SeriesOut>,
    pub other_matches: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TaskOut {
    pub task_id: String,
    pub product_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub supermarket_id: String,
    pub supermarket_name: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_original_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TasksOut {
    pub tasks: Vec<TaskOut>,
    pub count: usize,
    pub remaining_pending: i64,
    pub currency: String,
    pub timezone: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AlertPriceOut {
    pub supermarket_id: String,
    pub price: Money,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AlertTriggeredOut {
    pub product_name: String,
    pub target_price: Money,
    pub prices: Vec<AlertPriceOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FlaggedOut {
    pub task_id: String,
    pub product_name: String,
    pub supermarket_id: String,
    pub price: Money,
    pub last_price: Money,
    pub change_pct: f64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RejectedOut {
    pub task_id: String,
    pub reason: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SubmitOut {
    pub accepted: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alerts_triggered: Option<Vec<AlertTriggeredOut>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_sent_to: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notifications: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requeued: Option<i64>,
    pub flagged: Vec<FlaggedOut>,
    pub rejected: Vec<RejectedOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
    pub remaining_pending: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CheapestOut {
    pub supermarket_id: String,
    pub supermarket_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_price: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_review: Option<bool>,
    pub url: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AlertOut {
    pub product_name: String,
    pub target_price: Money,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supermarket_ids: Option<Vec<String>>,
    pub below_target_now: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cheapest: Option<CheapestOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SetAlertOut {
    #[serde(flatten)]
    pub alert: AlertOut,
    pub created: bool,
    pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AlertsOut {
    pub count: usize,
    pub alerts: Vec<AlertOut>,
    pub currency: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteAlertOut {
    pub product_name: String,
    pub deleted: bool,
    pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SetupMarketOut {
    pub supermarket_id: String,
    pub name: String,
    pub has_search_url_template: bool,
    pub has_schedule: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SetupMarketsOut {
    pub count: usize,
    pub items: Vec<SetupMarketOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SetupChannelOut {
    pub channel: String,
    pub enabled: bool,
    pub target_masked: String,
    pub last_test_ok: Option<bool>,
    pub last_test_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SetupStatusOut {
    pub supermarkets: SetupMarketsOut,
    pub products: i64,
    pub products_without_urls: i64,
    pub price_alerts: i64,
    pub notification_channels: Vec<SetupChannelOut>,
    pub cron_enabled: bool,
    pub mode: String,
    pub has_any_price: bool,
    pub currency: String,
    pub language: String,
    pub timezone: String,
    pub next_step: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PreferencesOut {
    pub currency: String,
    pub language: String,
    pub timezone: String,
    pub currency_locked: bool,
    pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SetChannelOut {
    pub channel: String,
    pub target_masked: Option<String>,
    pub enabled: bool,
    pub test_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
    pub message: String,
}
