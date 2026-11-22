// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use serde::de::{DeserializeOwned, Error};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

fn to_i64<E: Error>(v: &Value) -> Result<Option<i64>, E> {
    match v {
        Value::Null => Ok(None),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Some(i))
            } else if let Some(f) = n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9.0e15) {
                Ok(Some(f as i64))
            } else {
                Err(E::custom(format!("expected an integer, got {n}")))
            }
        }
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                return Ok(None);
            }
            t.parse::<i64>().map(Some).map_err(|_| E::custom(format!("expected an integer, got '{s}'")))
        }
        other => Err(E::custom(format!("expected an integer, got {other}"))),
    }
}

pub fn opt_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    to_i64(&Value::deserialize(d)?)
}

pub fn req_i64<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    to_i64(&Value::deserialize(d)?)?.ok_or_else(|| D::Error::custom("expected an integer"))
}

fn unwrap_json_string(v: Value) -> Value {
    match v {
        Value::String(s) if s.trim_start().starts_with('[') => serde_json::from_str(&s).unwrap_or(Value::String(s)),
        other => other,
    }
}

pub fn list<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    serde_json::from_value(unwrap_json_string(Value::deserialize(d)?)).map_err(D::Error::custom)
}

pub fn opt_list<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Option<Vec<T>>, D::Error> {
    match unwrap_json_string(Value::deserialize(d)?) {
        Value::Null => Ok(None),
        v => serde_json::from_value(v).map(Some).map_err(D::Error::custom),
    }
}

pub fn opt_bool<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    match Value::deserialize(d)? {
        Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(b)),
        Value::String(s) => crate::config::parse_bool(&s)
            .map(Some)
            .ok_or_else(|| D::Error::custom(format!("expected true or false, got '{s}'"))),
        other => Err(D::Error::custom(format!("expected true or false, got {other}"))),
    }
}
