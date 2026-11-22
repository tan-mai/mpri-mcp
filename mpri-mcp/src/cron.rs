// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

const MACROS: &[(&str, &str)] = &[
    ("@yearly", "0 0 1 1 *"),
    ("@annually", "0 0 1 1 *"),
    ("@monthly", "0 0 1 * *"),
    ("@weekly", "0 0 * * 0"),
    ("@daily", "0 0 * * *"),
    ("@midnight", "0 0 * * *"),
    ("@hourly", "0 * * * *"),
];
const DOW_NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
const MONTH_NAMES: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
const SEARCH_DAYS: i64 = 366 * 30;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CronError(pub String);

fn err<T>(msg: impl Into<String>) -> Result<T, CronError> {
    Err(CronError(msg.into()))
}

#[derive(Debug, Clone)]
pub struct Schedule {
    minutes: Vec<bool>,
    hours: Vec<bool>,
    days: Vec<bool>,
    last_day: bool,
    months: Vec<bool>,
    weekdays: [bool; 7],
    tz: Tz,
}

pub fn normalize_expression(expr: &str) -> String {
    let collapsed = expr.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = collapsed.to_lowercase();
    MACROS.iter().find(|(name, _)| *name == lower).map(|(_, value)| value.to_string()).unwrap_or(collapsed)
}

fn parse_num(s: &str) -> Option<u32> {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) { s.parse().ok() } else { None }
}

fn validate_range(
    expr: &str,
    first: u32,
    last: Option<u32>,
    step: Option<u32>,
    min: u32,
    max: u32,
) -> Result<(), CronError> {
    let wrap = |m: String| err(format!("Error validating expression '{expr}': {m}"));
    let total = max - min;
    if let Some(s) = step.filter(|s| *s > total) {
        return wrap(format!("the step value ({s}) is higher than the total range of the expression ({total})"));
    }
    if first < min {
        return wrap(format!("the first value ({first}) is lower than the minimum value ({min})"));
    }
    if let Some(l) = last.filter(|l| *l > max) {
        return wrap(format!("the last value ({l}) is higher than the maximum value ({max})"));
    }
    let range = last.unwrap_or(max).saturating_sub(first);
    if let Some(s) = step.filter(|s| *s > range) {
        return wrap(format!("the step value ({s}) is higher than the total range of the expression ({range})"));
    }
    Ok(())
}

fn compile_field(
    field: &str,
    name: &str,
    min: u32,
    max: u32,
    out: &mut [bool],
    last_day: Option<&mut bool>,
) -> Result<(), CronError> {
    let mut last_day = last_day;
    for raw in field.split(',') {
        let expr = raw.trim();
        let lower = expr.to_lowercase();
        if let Some(rest) = expr.strip_prefix('*') {
            let step = if rest.is_empty() {
                None
            } else {
                match rest.strip_prefix('/').and_then(parse_num) {
                    Some(s) => Some(s),
                    None => return err(format!("Unrecognized expression \"{expr}\" for field \"{name}\"")),
                }
            };
            if step == Some(0) {
                return err("Increment must be higher than 0");
            }
            validate_range(expr, min, None, step, min, max)?;
            let step = step.unwrap_or(1) as usize;
            for v in (min as usize..=max as usize).step_by(step) {
                out[v] = true;
            }
            continue;
        }
        if name == "day" && lower == "last" {
            if let Some(flag) = last_day.as_deref_mut() {
                *flag = true;
            }
            continue;
        }
        let (body, step) = match expr.split_once('/') {
            Some((b, s)) => match parse_num(s) {
                Some(n) => (b, Some(n)),
                None => return err(format!("Unrecognized expression \"{expr}\" for field \"{name}\"")),
            },
            None => (expr, None),
        };
        let (a, b) = match body.split_once('-') {
            Some((a, b)) => (a, Some(b)),
            None => (body, None),
        };
        let numeric = parse_num(a).is_some() && b.is_none_or(|b| parse_num(b).is_some());
        let (first, last) = if numeric {
            (parse_num(a).unwrap_or(0), b.and_then(parse_num))
        } else if name == "month" && step.is_none() && !a.is_empty() && a.chars().all(|c| c.is_ascii_alphabetic()) {
            let month = |s: &str| {
                MONTH_NAMES
                    .iter()
                    .position(|m| *m == s.to_lowercase())
                    .map(|p| p as u32 + 1)
                    .ok_or_else(|| CronError(format!("Invalid month name \"{s}\"")))
            };
            (month(a)?, b.map(month).transpose()?)
        } else {
            return err(format!("Unrecognized expression \"{expr}\" for field \"{name}\""));
        };
        let last = match (last, step) {
            (None, None) => Some(first),
            (l, _) => l,
        };
        if let Some(l) = last.filter(|l| first > *l) {
            let _ = l;
            return err("The minimum value in a range must not be higher than the maximum");
        }
        validate_range(expr, first, last, step, min, max)?;
        let end = last.unwrap_or(max);
        let step = step.unwrap_or(1).max(1) as usize;
        for v in (first as usize..=end as usize).step_by(step) {
            out[v] = true;
        }
    }
    Ok(())
}

fn dow_value(token: &str) -> Result<u32, CronError> {
    let lower = token.to_lowercase();
    if let Some(p) = DOW_NAMES.iter().position(|d| *d == lower) {
        return Ok(p as u32);
    }
    match parse_num(token) {
        Some(n) if n <= 7 => Ok(n % 7),
        _ => err(format!("invalid day-of-week value '{token}'")),
    }
}

fn expand_dow(field: &str) -> Result<[bool; 7], CronError> {
    let mut days = [false; 7];
    for part in field.split(',') {
        if part.is_empty() {
            return err("empty item in day-of-week field");
        }
        let (base, step_s) = match part.split_once('/') {
            Some((b, s)) => (b, Some(s)),
            None => (part, None),
        };
        let mut step = 1;
        if let Some(s) = step_s {
            match parse_num(s) {
                Some(n) if n > 0 => step = n,
                _ => return err(format!("invalid step '{s}' in day-of-week field")),
            }
        }
        let (start, end) = if base == "*" || base == "?" {
            (0, 6)
        } else if let Some((a, b)) = base.split_once('-') {
            let start = dow_value(a)?;
            let mut end = dow_value(b)?;
            if b == "7" {
                end = 7;
            }
            if end < start {
                return err(format!("descending range '{base}' in day-of-week field"));
            }
            (start, end)
        } else {
            let start = dow_value(base)?;
            (start, if step_s.is_some() { 6 } else { start })
        };
        let mut d = start;
        while d <= end {
            days[(d % 7) as usize] = true;
            d += step;
        }
    }
    Ok(days)
}

pub fn parse(expr: &str, timezone: &str) -> Result<Schedule, CronError> {
    let expr = normalize_expression(expr);
    let fields: Vec<&str> = expr.split(' ').filter(|f| !f.is_empty()).collect();
    if fields.len() != 5 {
        return err(format!("expected 5 fields (minute hour day month day-of-week), got {}: '{expr}'", fields.len()));
    }
    let tz: Tz = timezone.parse().map_err(|_| CronError(format!("unknown timezone '{timezone}'")))?;
    let mut s = Schedule {
        minutes: vec![false; 60],
        hours: vec![false; 24],
        days: vec![false; 32],
        last_day: false,
        months: vec![false; 13],
        weekdays: [false; 7],
        tz,
    };
    s.weekdays = expand_dow(fields[4])?;
    compile_field(fields[3], "month", 1, 12, &mut s.months, None)?;
    compile_field(fields[2], "day", 1, 31, &mut s.days, Some(&mut s.last_day))?;
    compile_field(fields[1], "hour", 0, 23, &mut s.hours, None)?;
    compile_field(fields[0], "minute", 0, 59, &mut s.minutes, None)?;
    Ok(s)
}

fn days_in_month(date: NaiveDate) -> u32 {
    let (y, m) = (date.year(), date.month());
    let next = if m == 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1) } else { NaiveDate::from_ymd_opt(y, m + 1, 1) };
    next.and_then(|n| n.pred_opt()).map(|d| d.day()).unwrap_or(28)
}

impl Schedule {
    fn day_matches(&self, date: NaiveDate) -> bool {
        let dom = date.day();
        let day_ok = self.days[dom as usize] || (self.last_day && dom == days_in_month(date));
        day_ok && self.months[date.month() as usize] && self.weekdays[date.weekday().num_days_from_sunday() as usize]
    }

    pub fn next_from(&self, start: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut start = start;
        if start.nanosecond() > 0 {
            start = start.with_nanosecond(0)? + Duration::seconds(1);
        }
        let local_start = start.with_timezone(&self.tz);
        let first_day = local_start.date_naive();
        for offset in 0..SEARCH_DAYS {
            let date = first_day + Duration::days(offset);
            if !self.day_matches(date) {
                continue;
            }
            for h in 0..24u32 {
                if !self.hours[h as usize] {
                    continue;
                }
                for m in 0..60u32 {
                    if !self.minutes[m as usize] {
                        continue;
                    }
                    let Some(naive) = date.and_hms_opt(h, m, 0) else { continue };
                    let Some(candidate) = self.tz.from_local_datetime(&naive).earliest() else {
                        continue;
                    };
                    let utc = candidate.with_timezone(&Utc);
                    if utc >= start {
                        return Some(utc);
                    }
                }
            }
        }
        None
    }

    pub fn next_times(&self, now: DateTime<Utc>, count: usize) -> Vec<DateTime<Utc>> {
        let mut out = Vec::new();
        let mut start = now;
        for _ in 0..count {
            let Some(next) = self.next_from(start) else { break };
            out.push(next);
            start = next + Duration::seconds(1);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_python_reference() {
        let raw = include_str!("../tests/fixtures/cron.json");
        let cases: Vec<serde_json::Value> = serde_json::from_str(raw).unwrap();
        for case in cases {
            let expr = case["expr"].as_str().unwrap();
            let now: DateTime<Utc> = case["now"].as_str().unwrap().parse().unwrap();
            let got = parse(expr, crate::clock::TIMEZONE);
            if let Some(expected) = case["error"].as_str() {
                assert_eq!(got.err().map(|e| e.0).as_deref(), Some(expected), "{expr}");
                continue;
            }
            let times: Vec<String> = got
                .unwrap_or_else(|e| panic!("{expr}: {e}"))
                .next_times(now, 5)
                .iter()
                .map(|t| t.to_rfc3339())
                .collect();
            let expected: Vec<String> = case["next"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().parse::<DateTime<Utc>>().unwrap().to_rfc3339())
                .collect();
            assert_eq!(times, expected, "{expr} from {now}");
        }
    }
}
