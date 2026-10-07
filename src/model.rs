use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub url: String,
    pub name: String,
    pub status: String,
    pub message: String,
    pub created: i64,
    pub updated: i64,
    pub attempt: u16,
    pub bytes: u64,
    pub output: Option<String>,
    #[serde(skip_serializing)]
    pub headers: BTreeMap<String, String>,
}
#[derive(Debug, Deserialize)]
pub struct AddRequest {
    pub urls: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn validate_url(raw: &str) -> anyhow::Result<url::Url> {
    let u = url::Url::parse(raw)?;
    anyhow::ensure!(
        matches!(u.scheme(), "http" | "https") && u.host_str().is_some(),
        "Only HTTP/HTTPS URLs are supported"
    );
    anyhow::ensure!(
        u.username().is_empty() && u.password().is_none(),
        "Provide authentication through request headers"
    );
    Ok(u)
}
pub fn safe_name(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(70)
        .collect();
    let s = s.trim_matches([' ', '.']);
    if s.is_empty() {
        "video".into()
    } else {
        s.into()
    }
}
