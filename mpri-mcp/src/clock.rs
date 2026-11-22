// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

pub const TIMEZONE: &str = "Asia/Ho_Chi_Minh";
pub const TZ: Tz = chrono_tz::Asia::Ho_Chi_Minh;

pub const MINUTE_MS: i64 = 60_000;
pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 86_400_000;

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub fn to_utc(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default()
}

pub fn local(ms: i64) -> DateTime<Tz> {
    to_utc(ms).with_timezone(&TZ)
}

pub fn human(ms: Option<i64>) -> Option<String> {
    ms.map(|v| local(v).format("%a %d/%m/%Y %H:%M").to_string())
}

pub fn fmt_local(ms: Option<i64>, fmt: &str) -> String {
    ms.map(|v| local(v).format(fmt).to_string()).unwrap_or_else(|| "—".into())
}

pub fn local_date(ms: i64) -> NaiveDate {
    local(ms).date_naive()
}

pub fn ago(ms: Option<i64>) -> String {
    let Some(ms) = ms else { return "chưa bao giờ".into() };
    let seconds = (now_ms() - ms) / 1000;
    let (future, s) = if seconds < 0 { (true, -seconds) } else { (false, seconds) };
    for (unit, size) in [("ngày", 86_400), ("giờ", 3_600), ("phút", 60)] {
        if s >= size {
            let n = s / size;
            return if future { format!("sau {n} {unit}") } else { format!("{n} {unit} trước") };
        }
    }
    if future { "sắp tới".into() } else { "vừa xong".into() }
}
