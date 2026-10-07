pub mod browser;
pub mod text;
use crate::{config::Config, model::validate_url};
use anyhow::{Context, Result};
use reqwest::{
    Client,
    cookie::{CookieStore, Jar},
    header::{HeaderMap, HeaderName, HeaderValue},
};
use scraper::{Html, Selector};
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};
use url::Url;

pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
#[derive(Clone, Debug)]
pub struct Resolved {
    pub url: Url,
    pub headers: BTreeMap<String, String>,
    pub duration: Option<f64>,
}
fn request_headers(headers: &BTreeMap<String, String>) -> Result<HeaderMap> {
    let mut out = HeaderMap::new();
    for (k, v) in headers {
        out.insert(
            HeaderName::from_bytes(k.as_bytes())?,
            HeaderValue::from_str(v)?,
        );
    }
    Ok(out)
}
pub fn normalize_headers(headers: BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    anyhow::ensure!(
        headers.len() <= 20,
        "At most 20 request headers are allowed"
    );
    let mut out = BTreeMap::new();
    for (k, v) in headers {
        let k = k.to_ascii_lowercase();
        anyhow::ensure!(
            !["host", "content-length", "connection", "transfer-encoding"].contains(&k.as_str()),
            "Cannot override request header {k}"
        );
        anyhow::ensure!(v.len() <= 8192, "Request header value is too long");
        out.insert(k, v);
    }
    request_headers(&out)?;
    Ok(out)
}
async fn body_limited(resp: reqwest::Response) -> Result<String> {
    use futures_util::StreamExt;
    let mut data = Vec::new();
    let mut stream = resp.error_for_status()?.bytes_stream();
    while let Some(c) = stream.next().await {
        let c = c?;
        anyhow::ensure!(
            data.len() + c.len() <= 4 * 1024 * 1024,
            "Page/playlist exceeds the 4 MB limit"
        );
        data.extend_from_slice(&c);
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}
fn context_headers(
    input: &BTreeMap<String, String>,
    referer: &str,
    jar: &Jar,
    u: &Url,
) -> BTreeMap<String, String> {
    let mut h = input.clone();
    h.entry("user-agent".into()).or_insert(UA.into());
    h.entry("referer".into()).or_insert(referer.into());
    if let Some(c) = jar.cookies(u)
        && let Ok(c) = c.to_str()
    {
        let original = h.remove("cookie").unwrap_or_default();
        h.insert(
            "cookie".into(),
            if original.is_empty() {
                c.into()
            } else {
                format!("{original}; {c}")
            },
        );
    }
    h
}
pub async fn validate(
    client: &Client,
    u: Url,
    headers: BTreeMap<String, String>,
) -> Result<Resolved> {
    let mut current = u.clone();
    let mut duration = None;
    for _ in 0..4 {
        let response = client
            .get(current.clone())
            .headers(request_headers(&headers)?)
            .send()
            .await?;
        current = response.url().clone();
        let txt = body_limited(response).await?;
        let t = txt.trim_start_matches('\u{feff}').trim_start();
        anyhow::ensure!(
            t.starts_with("#EXTM3U") || (t.contains("<MPD") && !t.contains("<html")),
            "Candidate is not a valid HLS/DASH playlist"
        );
        if !t.starts_with("#EXTM3U") {
            return Ok(Resolved {
                url: u,
                headers,
                duration,
            });
        }
        let mut bandwidth = 0u64;
        let mut variants = Vec::new();
        for line in t.lines() {
            if let Some(properties) = line.strip_prefix("#EXT-X-STREAM-INF:") {
                bandwidth = properties
                    .split(',')
                    .find_map(|p| {
                        p.strip_prefix("BANDWIDTH=")
                            .and_then(|v| v.parse::<u64>().ok())
                    })
                    .unwrap_or(1);
            } else if !line.starts_with('#') && !line.trim().is_empty() && bandwidth > 0 {
                variants.push((bandwidth, current.join(line.trim())?));
                bandwidth = 0;
            }
        }
        if let Some((_, variant)) = variants.into_iter().max_by_key(|(b, _)| *b) {
            current = variant;
            continue;
        }
        anyhow::ensure!(
            t.contains("#EXTINF"),
            "HLS playlist has no valid video segments"
        );
        anyhow::ensure!(
            t.contains("#EXT-X-ENDLIST"),
            "Live playlist detected. This version supports on-demand video only."
        );
        let seconds: f64 = t
            .lines()
            .filter_map(|l| {
                l.strip_prefix("#EXTINF:")
                    .and_then(|v| v.split(',').next())
                    .and_then(|v| v.parse::<f64>().ok())
            })
            .sum();
        anyhow::ensure!(
            seconds.is_finite() && seconds > 0.0,
            "Invalid playlist duration"
        );
        duration = Some(seconds);
        return Ok(Resolved {
            url: u,
            headers,
            duration,
        });
    }
    anyhow::bail!("Master playlist nesting exceeds the limit")
}

pub async fn resolve(
    config: &Config,
    raw: &str,
    input: &BTreeMap<String, String>,
) -> Result<Resolved> {
    tokio::time::timeout(Duration::from_secs(90), resolve_inner(config, raw, input))
        .await
        .context("Parsing exceeded 90 seconds")?
}
async fn resolve_inner(
    config: &Config,
    raw: &str,
    input: &BTreeMap<String, String>,
) -> Result<Resolved> {
    let initial = validate_url(raw)?;
    let jar = Arc::new(Jar::default());
    let client = Client::builder()
        .cookie_provider(jar.clone())
        .timeout(Duration::from_secs(12))
        .user_agent(UA)
        .build()?;
    let mut pending = VecDeque::from([(initial.clone(), 0)]);
    let mut seen = HashSet::new();
    let mut errors = Vec::new();
    let mut page_ref = raw.to_owned();
    let static_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    'static_pages: while let Some((u, depth)) = pending.pop_front() {
        if tokio::time::Instant::now() >= static_deadline {
            break;
        }
        if !seen.insert(u.to_string()) || seen.len() > 12 {
            continue;
        }
        let response = match client
            .get(u.clone())
            .headers(request_headers(input)?)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                errors.push(e.to_string());
                continue;
            }
        };
        let final_url = response.url().clone();
        if depth == 0 {
            page_ref = final_url.to_string();
        }
        let body = match body_limited(response).await {
            Ok(b) => b,
            Err(e) => {
                errors.push(e.to_string());
                continue;
            }
        };
        let h = context_headers(input, &page_ref, &jar, &final_url);
        if (body.trim_start().starts_with("#EXTM3U") || body.contains("<MPD"))
            && let Ok(mut r) = validate(&client, final_url.clone(), h).await
        {
            r.headers = context_headers(input, &page_ref, &jar, &r.url);
            return Ok(r);
        }
        for candidate in text::extract(&body, &final_url).into_iter().take(12) {
            if tokio::time::Instant::now() >= static_deadline {
                break 'static_pages;
            }
            let h = context_headers(input, &page_ref, &jar, &candidate);
            match validate(&client, candidate, h).await {
                Ok(mut r) => {
                    r.headers = context_headers(input, &page_ref, &jar, &r.url);
                    return Ok(r);
                }
                Err(e) => errors.push(e.to_string()),
            }
        }
        if depth < 2 {
            // HTML 对象不跨越 await（scraper DOM 不是 Send）。
            let refs = {
                let dom = Html::parse_document(&body);
                let selector = Selector::parse("iframe[src],script[src]").unwrap();
                dom.select(&selector)
                    .filter_map(|e| {
                        let s = e.value().attr("src")?;
                        let v = final_url.join(s).ok()?;
                        if e.value().name() == "script" && v.origin() != final_url.origin() {
                            return None;
                        }
                        if !matches!(v.scheme(), "http" | "https") {
                            return None;
                        }
                        Some(v)
                    })
                    .take(8)
                    .collect::<Vec<_>>()
            };
            pending.extend(refs.into_iter().map(|v| (v, depth + 1)));
        }
    }
    match browser::capture(config, raw, input).await {
        Ok(found) => {
            for (u, h) in found {
                match validate(&client, u, h).await {
                    Ok(r) => return Ok(r),
                    Err(e) => errors.push(e.to_string()),
                }
            }
        }
        Err(e) => errors.push(format!("Browser parsing: {e:#}")),
    }
    anyhow::bail!(
        "No valid media playlist found. {}",
        errors
            .into_iter()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .join("；")
    )
}
