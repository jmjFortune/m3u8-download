use base64::Engine;
use regex::Regex;
use std::collections::{HashSet, VecDeque};
use url::Url;

/// Follow only an explicitly advertised gotoPath destination carrying the old page path.
/// No JavaScript is executed, and comments or unrelated links are not navigation targets.
pub fn page_continuations(text: &str, base: &Url) -> Vec<Url> {
    let Some((_, path)) = base.query_pairs().find(|(key, _)| key == "path") else {
        return Vec::new();
    };
    if !text.contains("gotoPath")
        || !path.starts_with('/')
        || path.starts_with("//")
        || path.contains('\\')
        || path.len() > 4096
    {
        return Vec::new();
    }
    let dom = scraper::Html::parse_document(text);
    let selector = scraper::Selector::parse("[onclick]").unwrap();
    let call = Regex::new(r#"^\s*gotoPath\(\s*['"](https?://[^'"\s]+)['"]\s*\)\s*;?\s*$"#).unwrap();
    let mut seen = HashSet::new();
    dom.select(&selector)
        .filter_map(|element| {
            let captures = call.captures(element.value().attr("onclick")?)?;
            let target = Url::parse(&captures[1]).ok()?;
            if !matches!(target.scheme(), "http" | "https")
                || !target.username().is_empty()
                || target.password().is_some()
                || target.query().is_some()
                || target.fragment().is_some()
            {
                return None;
            }
            let next = Url::parse(&format!(
                "{}{}",
                target.as_str().trim_end_matches('/'),
                path
            ))
            .ok()?;
            seen.insert(next.to_string()).then_some(next)
        })
        .take(4)
        .collect()
}

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

    #[tokio::test]
    async fn resolve_page_that_moved_through_a_publisher() {
        use axum::{
            Router,
            response::{Html, Redirect},
            routing::get,
        };
        use clap::Parser;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let publisher = format!(r#"<button onclick="gotoPath('{origin}/moved');">Enter</button>"#);
        let app = Router::new()
            .route(
                "/old/play/demo",
                get(|| async {
                    Redirect::temporary("/published?path=%2Fplay%2Fdemo%3Fsig%3Da%252Fb")
                }),
            )
            .route(
                "/published",
                get(move || {
                    let p = publisher.clone();
                    async { Html(p) }
                }),
            )
            .route(
                "/moved/play/demo",
                get(|| async {
                    "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nvideo.ts\n#EXT-X-ENDLIST\n"
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config =
            crate::config::Config::parse_from(["pagecatch", "--browser", "/no-browser-needed"]);
        let result = crate::resolver::resolve(
            &config,
            &format!("{origin}/old/play/demo"),
            &Default::default(),
        )
        .await;
        server.abort();
        let resolved = result.expect("Published migration should resolve without a browser");
        assert_eq!(
            resolved.url.as_str(),
            format!("{origin}/moved/play/demo?sig=a%2Fb")
        );
        assert_eq!(resolved.duration, Some(2.0));
        assert_eq!(
            resolved.headers["referer"],
            format!("{origin}/moved/play/demo?sig=a%2Fb")
        );
    }

    #[test]
    fn publisher_navigation_preserves_signed_path_and_ignores_unrelated_targets() {
        let base = Url::parse("https://publisher.test/?path=%2Fplay%2Fid%3Fsig%3Da%252Fb").unwrap();
        let body = r#"<!-- <button onclick="gotoPath('https://comment.test')"> -->
            <div onclick="gotoPath('https://new.test:8888');"></div>
            <div onclick="gotoPath('https://new.test:8888');"></div>
            <a href="https://ads.test">Ad</a>
            <div onclick="gotoPath('https://user:pass@credentials.test')"></div>"#;
        let urls = page_continuations(body, &base);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].as_str(), "https://new.test:8888/play/id?sig=a%2Fb");
        for invalid in [
            "https://publisher.test/?path=%2F%2Fother.test",
            "https://publisher.test/?path=%2F%5Cother.test",
            "https://publisher.test/play/id",
        ] {
            assert!(page_continuations(body, &Url::parse(invalid).unwrap()).is_empty());
        }
    }
}
