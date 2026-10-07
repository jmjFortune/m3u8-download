use base64::Engine;
use regex::Regex;
use std::collections::{HashSet, VecDeque};
use url::Url;

/// 解码只作用于静态文本，不执行页面中的 JavaScript。
pub fn extract(text: &str, base: &Url) -> Vec<Url> {
    let absolute = Regex::new(r#"(?i)https?://[^\s"'<>\\]+?\.(?:m3u8|mpd)[^\s"'<>\\]*"#).unwrap();
    let quoted = Regex::new(r#"["']([^\s"'<>]*?\.(?:m3u8|mpd)[^\s"'<>]*)["']"#).unwrap();
    let b64 = Regex::new(r#"(?:atob\s*\(\s*)?["']([A-Za-z0-9+/]{16,}={0,2})["']"#).unwrap();
    let unicode = Regex::new(r"\\(?:u00|x)([0-9a-fA-F]{2})").unwrap();
    let mut queue = VecDeque::from([(text.to_owned(), 0)]);
    let mut layers = HashSet::new();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    while let Some((s, depth)) = queue.pop_front() {
        let s = html_escape::decode_html_entities(
            &s.replace("\\/", "/")
                .replace("\\\"", "\"")
                .replace("\\'", "'"),
        )
        .into_owned();
        if !layers.insert(s.clone()) || layers.len() > 24 {
            continue;
        }
        for raw in absolute.find_iter(&s).map(|m| m.as_str()).chain(
            quoted
                .captures_iter(&s)
                .filter_map(|c| c.get(1).map(|m| m.as_str())),
        ) {
            if raw.contains("\\")
                || (raw.to_lowercase().starts_with("http%")
                    || raw.to_lowercase().starts_with("https%"))
            {
                continue;
            }
            let raw = raw.trim_end_matches([',', ';', ')']);
            if let Ok(u) = base.join(raw)
                && matches!(u.scheme(), "http" | "https")
                && seen.insert(u.to_string())
            {
                out.push(u);
            }
        }
        if depth >= 6 {
            continue;
        }
        let decoded = unicode
            .replace_all(&s, |c: &regex::Captures| {
                char::from(u8::from_str_radix(&c[1], 16).unwrap()).to_string()
            })
            .replace("\\/", "/")
            .replace("\\\"", "\"")
            .replace("\\'", "'");
        let decoded = html_escape::decode_html_entities(&decoded).into_owned();
        if decoded != s {
            queue.push_back((decoded, depth + 1));
        }
        let decoded = percent_encoding::percent_decode_str(&s)
            .decode_utf8_lossy()
            .into_owned();
        if decoded != s {
            queue.push_back((decoded, depth + 1));
        }
        for c in b64.captures_iter(&s).take(20) {
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&c[1])
                && let Ok(t) = String::from_utf8(bytes)
            {
                queue.push_back((t, depth + 1));
            }
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoded_and_relative_addresses() {
        let b = Url::parse("https://site.test/watch/page").unwrap();
        for raw in [
            r#""https:\/\/cdn.test\/a.m3u8?x=1&amp;y=2""#,
            r#"decodeURIComponent("https%253A%252F%252Fcdn.test%252Fa.m3u8%253Fx%253D1%2526y%253D2")"#,
        ] {
            assert_eq!(
                extract(raw, &b)[0].as_str(),
                "https://cdn.test/a.m3u8?x=1&y=2"
            );
        }
        assert_eq!(
            extract("'../a.m3u8'", &b)[0].as_str(),
            "https://site.test/a.m3u8"
        );
        assert_eq!(
            extract(r#"atob("aHR0cHM6Ly9jZG4udGVzdC9hLm0zdTg=")"#, &b)[0].as_str(),
            "https://cdn.test/a.m3u8"
        );
    }
}
