// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;
use url::Url;

pub fn strip_diacritics(text: &str) -> String {
    text.replace('đ', "d")
        .replace('Đ', "D")
        .nfd()
        .filter(|c| get_general_category(*c) != GeneralCategory::NonspacingMark)
        .collect()
}

pub fn normalize_name(text: &str) -> String {
    let lowered = strip_diacritics(text).to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut gap = false;
    for c in lowered.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
pub fn name_key(text: &str) -> String {
    strip_diacritics(&collapse_spaces(text)).to_lowercase()
}

pub fn slugify(text: &str) -> String {
    normalize_name(text).replace(' ', "-")
}

pub fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn netloc(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default().to_lowercase();
    match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    }
}

pub fn normalize_base_url(raw: &str) -> Result<String, String> {
    let mut url = raw.trim().to_string();
    if url.is_empty() {
        return Err("URL is empty".into());
    }
    if !url.contains("://") {
        url = format!("https://{url}");
    }
    let invalid = || format!("'{url}' is not a valid http(s) URL");
    let parsed = Url::parse(&url).map_err(|_| invalid())?;
    let loc = netloc(&parsed);
    if !matches!(parsed.scheme(), "http" | "https") || loc.is_empty() || !loc.contains('.') {
        return Err(invalid());
    }
    Ok(format!("{}://{}{}", parsed.scheme(), loc, parsed.path().trim_end_matches('/')))
}

pub fn validate_page_url(raw: &str) -> Result<String, String> {
    let url = raw.trim().to_string();
    let ok =
        Url::parse(&url).map(|u| matches!(u.scheme(), "http" | "https") && !netloc(&u).is_empty()).unwrap_or(false);
    if ok { Ok(url) } else { Err(format!("'{url}' is not a valid http(s) URL")) }
}

pub fn registrable_host(url: &str) -> String {
    let full = if url.contains("://") { url.to_string() } else { format!("https://{url}") };
    let host = Url::parse(&full).ok().and_then(|u| u.host_str().map(str::to_lowercase)).unwrap_or_default();
    host.strip_prefix("www.").map(str::to_string).unwrap_or(host)
}

pub fn same_site(url: &str, base_url: &str) -> bool {
    let host = registrable_host(url);
    let base = registrable_host(base_url);
    host == base || host.ends_with(&format!(".{base}")) || base.ends_with(&format!(".{host}"))
}

fn lcs_len(a: &[char], b: &[char]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let mut prev = vec![0usize; b.len() + 1];
    let mut cur = vec![0usize; b.len() + 1];
    for ca in a {
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = if ca == cb { prev[j] + 1 } else { prev[j + 1].max(cur[j]) };
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn indel_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    a.len() + b.len() - 2 * lcs_len(&a, &b)
}

fn norm_distance(dist: usize, lensum: usize) -> f64 {
    if lensum == 0 { 100.0 } else { 100.0 - 100.0 * dist as f64 / lensum as f64 }
}

pub fn ratio(a: &str, b: &str) -> f64 {
    let lensum = a.chars().count() + b.chars().count();
    norm_distance(indel_distance(a, b), lensum)
}

fn sorted_tokens(s: &str) -> Vec<&str> {
    let mut tokens: Vec<&str> = s.split_whitespace().collect();
    tokens.sort_unstable();
    tokens
}

pub fn token_sort_ratio(a: &str, b: &str) -> f64 {
    ratio(&sorted_tokens(a).join(" "), &sorted_tokens(b).join(" "))
}

pub fn token_set_ratio(a: &str, b: &str) -> f64 {
    let mut ta = sorted_tokens(a);
    let mut tb = sorted_tokens(b);
    ta.dedup();
    tb.dedup();
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let intersect: Vec<&str> = ta.iter().copied().filter(|t| tb.contains(t)).collect();
    let diff_ab: Vec<&str> = ta.iter().copied().filter(|t| !tb.contains(t)).collect();
    let diff_ba: Vec<&str> = tb.iter().copied().filter(|t| !ta.contains(t)).collect();
    if !intersect.is_empty() && (diff_ab.is_empty() || diff_ba.is_empty()) {
        return 100.0;
    }
    let ab = diff_ab.join(" ");
    let ba = diff_ba.join(" ");
    let ab_len = ab.chars().count();
    let ba_len = ba.chars().count();
    let sect_len = intersect.join(" ").chars().count();
    let has_sect = usize::from(sect_len > 0);
    let sect_ab_len = sect_len + has_sect + ab_len;
    let sect_ba_len = sect_len + has_sect + ba_len;
    let result = norm_distance(indel_distance(&ab, &ba), sect_ab_len + sect_ba_len);
    if sect_len == 0 {
        return result;
    }
    let sect_ab = norm_distance(has_sect + ab_len, sect_len + sect_ab_len);
    let sect_ba = norm_distance(has_sect + ba_len, sect_len + sect_ba_len);
    result.max(sect_ab).max(sect_ba)
}

pub fn round1(value: f64) -> f64 {
    format!("{value:.1}").parse().unwrap_or(value)
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatch<K> {
    pub key: K,
    pub name: String,
    pub score: f64,
}

pub fn fuzzy_rank<K: Clone>(
    query: &str,
    candidates: impl IntoIterator<Item = (K, String)>,
    limit: usize,
    min_score: f64,
) -> Vec<FuzzyMatch<K>> {
    let q = normalize_name(query);
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(f64, f64, usize, FuzzyMatch<K>)> = Vec::new();
    for (idx, (key, name)) in candidates.into_iter().enumerate() {
        let n = normalize_name(&name);
        let primary = if n == q { 100.0 } else { token_set_ratio(&q, &n) };
        if primary < min_score {
            continue;
        }
        let tiebreak = token_sort_ratio(&q, &n);
        scored.push((primary, tiebreak, idx, FuzzyMatch { key, name, score: round1(primary) }));
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.2.cmp(&b.2))
    });
    scored.into_iter().take(limit).map(|(_, _, _, m)| m).collect()
}

pub fn fmt_thousands(value: i64, sep: char) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    if value < 0 { format!("-{out}") } else { out }
}

pub fn fmt_g(value: f64) -> String {
    if value.fract() == 0.0 { format!("{}", value as i64) } else { format!("{value}") }
}

pub fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 9 {
        return "***".into();
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[chars.len() - 3..].iter().collect();
    format!("{head}***{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_rapidfuzz_reference() {
        let raw = include_str!("../tests/fixtures/fuzzy.json");
        let data: serde_json::Value = serde_json::from_str(raw).unwrap();
        for p in data["pairs"].as_array().unwrap() {
            let (q, n) = (p["q"].as_str().unwrap(), p["n"].as_str().unwrap());
            for (key, got) in [
                ("token_set_ratio", token_set_ratio(q, n)),
                ("token_sort_ratio", token_sort_ratio(q, n)),
                ("ratio", ratio(q, n)),
            ] {
                let want = p[key].as_f64().unwrap();
                assert!((got - want).abs() < 1e-9, "{key}({q:?}, {n:?}) = {got}, want {want}");
            }
        }
        for t in data["normalize"].as_array().unwrap() {
            let text = t["text"].as_str().unwrap();
            assert_eq!(normalize_name(text), t["normalize_name"].as_str().unwrap(), "{text:?}");
            assert_eq!(slugify(text), t["slugify"].as_str().unwrap(), "{text:?}");
        }
        for r in data["ranks"].as_array().unwrap() {
            let query = r["query"].as_str().unwrap();
            let candidates: Vec<(i64, String)> =
                CANDIDATES.iter().enumerate().map(|(i, n)| (i as i64, n.to_string())).collect();
            let got = fuzzy_rank(query, candidates, 5, 60.0);
            let want = r["results"].as_array().unwrap();
            assert_eq!(got.len(), want.len(), "{query:?}");
            for (g, w) in got.iter().zip(want) {
                assert_eq!(g.key, w["key"].as_i64().unwrap(), "{query:?}");
                assert!((g.score - w["score"].as_f64().unwrap()).abs() < 1e-9, "{query:?}");
            }
        }
    }

    const CANDIDATES: [&str; 17] = [
        "Sữa TH True Milk Ít Đường 110ml Thùng 48 Hộp",
        "Sữa TH True Milk Có Đường 110ml Lốc 4 Hộp",
        "Sữa tươi Vinamilk 100% Không Đường 1L",
        "Nước mắm Nam Ngư 500ml",
        "Gạo ST25 Ông Cua 5kg",
        "Dầu ăn Neptune Gold 1 lít",
        "Mì Hảo Hảo tôm chua cay thùng 30 gói",
        "Nước suối Lavie 500ml lốc 6 chai",
        "Bia Tiger Crystal lon 330ml thùng 24",
        "Trứng gà Ba Huân hộp 10 quả",
        "Đường tinh luyện Biên Hòa 1kg",
        "Cà phê G7 3in1 hộp 21 gói",
        "Bách Hóa Xanh",
        "Co.opmart",
        "WinMart+",
        "LOTTE Mart",
        "Satori nước khoáng 500ml",
    ];

    #[test]
    fn masks_secrets() {
        assert_eq!(mask_secret("123456789:AAbbccddeeffgghhiijjkkllmmnnooppqq_xyz"), "123456***xyz");
        assert_eq!(mask_secret("short"), "***");
    }
}
