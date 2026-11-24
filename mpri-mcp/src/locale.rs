// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Serialize, Serializer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Currency {
    Vnd,
    Usd,
}

impl Currency {
    pub const ALL: [Currency; 2] = [Currency::Vnd, Currency::Usd];

    pub fn code(self) -> &'static str {
        match self {
            Currency::Vnd => "VND",
            Currency::Usd => "USD",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.code().eq_ignore_ascii_case(raw.trim()))
    }

    pub fn exponent(self) -> u32 {
        match self {
            Currency::Vnd => 0,
            Currency::Usd => 2,
        }
    }

    fn factor(self) -> i64 {
        10_i64.pow(self.exponent())
    }

    pub fn example(self) -> &'static str {
        match self {
            Currency::Vnd => "70000",
            Currency::Usd => "3.50",
        }
    }

    pub fn to_minor(self, value: f64) -> Result<i64, String> {
        if !value.is_finite() {
            return Err(format!("{value} is not a number."));
        }
        let scaled = value * self.factor() as f64;
        let rounded = scaled.round();
        if (scaled - rounded).abs() > 1e-6 {
            return Err(match self {
                Currency::Vnd => format!("{value} must be a whole number of VND (VND has no decimals)."),
                Currency::Usd => format!("{value} has more than 2 decimals for USD."),
            });
        }
        if rounded.abs() > 9.0e15 {
            return Err(format!("{value} is too large."));
        }
        Ok(rounded as i64)
    }

    pub fn money(self, minor: i64) -> Money {
        Money { minor, currency: self }
    }

    fn grouped(self, minor: i64, thousands: char, decimal: char) -> String {
        let factor = self.factor();
        let whole = crate::text::fmt_thousands(minor.abs() / factor, thousands);
        let sign = if minor < 0 { "-" } else { "" };
        match self.exponent() {
            0 => format!("{sign}{whole}"),
            exp => format!("{sign}{whole}{decimal}{:0width$}", minor.abs() % factor, width = exp as usize),
        }
    }

    pub fn format(self, minor: i64) -> String {
        match self {
            Currency::Vnd => format!("{} ₫", self.grouped(minor, '.', ',')),
            Currency::Usd => format!("${}", self.grouped(minor, ',', '.')),
        }
    }

    pub fn format_opt(self, minor: Option<i64>) -> String {
        minor.map(|m| self.format(m)).unwrap_or_else(|| "—".into())
    }

    pub fn format_code(self, minor: i64) -> String {
        match self {
            Currency::Vnd => format!("{} VND", self.grouped(minor, ',', '.')),
            Currency::Usd => format!("${}", self.grouped(minor, ',', '.')),
        }
    }

    pub fn parse_input(self, raw: &str) -> Option<i64> {
        let cleaned: String = match self {
            Currency::Vnd => raw.chars().filter(|c| !matches!(c, '.' | ',' | ' ' | '₫' | 'đ' | 'Đ')).collect(),
            Currency::Usd => raw.chars().filter(|c| !matches!(c, '$' | ',' | ' ')).collect(),
        };
        let value: f64 = cleaned.parse().ok()?;
        self.to_minor(value).ok()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Money {
    pub minor: i64,
    pub currency: Currency,
}

impl Serialize for Money {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.currency.exponent() {
            0 => s.serialize_i64(self.minor),
            _ => s.serialize_f64(self.minor as f64 / self.currency.factor() as f64),
        }
    }
}

impl JsonSchema for Money {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Money".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "number" })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Vi,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Vi];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Vi => "vi",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.code().eq_ignore_ascii_case(raw.trim()))
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Vi => "Tiếng Việt",
        }
    }
}

pub fn t(lang: Lang, en: &'static str, vi: &'static str) -> &'static str {
    match lang {
        Lang::En => en,
        Lang::Vi => vi,
    }
}

pub fn tf(lang: Lang, en: String, vi: String) -> String {
    match lang {
        Lang::En => en,
        Lang::Vi => vi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_and_formats_money() {
        assert_eq!(Currency::Vnd.to_minor(45900.0), Ok(45900));
        assert!(Currency::Vnd.to_minor(45.9).is_err());
        assert_eq!(Currency::Usd.to_minor(3.99), Ok(399));
        assert_eq!(Currency::Usd.to_minor(0.1 + 0.2), Ok(30));
        assert!(Currency::Usd.to_minor(3.999).is_err());
        assert_eq!(Currency::Vnd.format(399000), "399.000 ₫");
        assert_eq!(Currency::Usd.format(123456), "$1,234.56");
        assert_eq!(Currency::Usd.format(5), "$0.05");
        assert_eq!(Currency::Vnd.format_code(70000), "70,000 VND");
        assert_eq!(Currency::Usd.parse_input("$1,234.5"), Some(123450));
        assert_eq!(Currency::Vnd.parse_input("70.000 ₫"), Some(70000));
        assert_eq!(serde_json::to_string(&Currency::Usd.money(399)).unwrap(), "3.99");
        assert_eq!(serde_json::to_string(&Currency::Vnd.money(45900)).unwrap(), "45900");
    }
}
