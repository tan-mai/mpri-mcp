// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

use crate::locale::Lang;

pub const DEFAULT_TIMEZONE: &str = "Asia/Ho_Chi_Minh";

pub const MINUTE_MS: i64 = 60_000;
pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 86_400_000;

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub fn to_utc(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default()
}

pub fn local(ms: i64, tz: Tz) -> DateTime<Tz> {
    to_utc(ms).with_timezone(&tz)
}

pub fn human(ms: Option<i64>, tz: Tz) -> Option<String> {
    ms.map(|v| local(v, tz).format("%a %d/%m/%Y %H:%M").to_string())
}

pub fn fmt_local(ms: Option<i64>, fmt: &str, tz: Tz) -> String {
    ms.map(|v| local(v, tz).format(fmt).to_string()).unwrap_or_else(|| "—".into())
}

pub fn short_datetime(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "%b %d %H:%M",
        Lang::Vi => "%d/%m %H:%M",
    }
}

pub fn long_datetime(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "%b %d, %Y %H:%M",
        Lang::Vi => "%d/%m/%Y %H:%M",
    }
}

pub fn short_date(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "%b %d",
        Lang::Vi => "%d/%m",
    }
}

pub fn local_date(ms: i64, tz: Tz) -> NaiveDate {
    local(ms, tz).date_naive()
}

pub fn ago(ms: Option<i64>, lang: Lang) -> String {
    let Some(ms) = ms else {
        return match lang {
            Lang::En => "never".into(),
            Lang::Vi => "chưa bao giờ".into(),
        };
    };
    let seconds = (now_ms() - ms) / 1000;
    let (future, s) = if seconds < 0 { (true, -seconds) } else { (false, seconds) };
    let units: [(&str, &str, i64); 3] = [("d", "ngày", 86_400), ("h", "giờ", 3_600), ("min", "phút", 60)];
    for (en, vi, size) in units {
        if s >= size {
            let n = s / size;
            return match (lang, future) {
                (Lang::En, true) => format!("in {n} {en}"),
                (Lang::En, false) => format!("{n} {en} ago"),
                (Lang::Vi, true) => format!("sau {n} {vi}"),
                (Lang::Vi, false) => format!("{n} {vi} trước"),
            };
        }
    }
    match (lang, future) {
        (Lang::En, true) => "soon".into(),
        (Lang::En, false) => "just now".into(),
        (Lang::Vi, true) => "sắp tới".into(),
        (Lang::Vi, false) => "vừa xong".into(),
    }
}
