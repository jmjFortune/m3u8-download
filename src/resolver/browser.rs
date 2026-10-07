use crate::config::Config;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};
use tokio::process::Command;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

pub fn executable(config: &Config) -> Option<PathBuf> {
    if let Some(ref p) = config.browser {
        return Some(p.clone());
    }
    let mut paths = vec![
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/usr/bin/chromium-browser"),
    ];
    for env in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
        if let Ok(p) = std::env::var(env) {
            paths.push(PathBuf::from(p).join("Google/Chrome/Application/chrome.exe"));
        }
    }
    paths.into_iter().find(|p| p.is_file())
}
/// 独立临时浏览器配置目录，避免读取用户浏览器会话。
pub async fn capture(
    config: &Config,
    page: &str,
    headers: &BTreeMap<String, String>,
) -> Result<Vec<(Url, BTreeMap<String, String>)>> {
    let exe = executable(config)
        .context("Chrome/Chromium not found. Install a browser or set PC_BROWSER.")?;
    let profile = tempfile::tempdir()?;
    let mut cmd = Command::new(exe);
    cmd.args([
        "--headless=new",
        "--remote-debugging-port=0",
        "--remote-allow-origins=*",
        "--no-first-run",
        "--no-default-browser-check",
        "--autoplay-policy=no-user-gesture-required",
    ])
    .arg(format!("--user-data-dir={}", profile.path().display()))
    .arg("about:blank")
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .kill_on_drop(true);
    if config.browser_no_sandbox {
        cmd.arg("--no-sandbox");
    }
    let mut child = cmd.spawn().context("Could not start Chromium")?;
    let result = tokio::time::timeout(
        Duration::from_secs(35),
        capture_session(profile.path(), &mut child, page, headers),
    )
    .await
    .context("Browser parsing timed out")?;
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}

async fn capture_session(
    profile: &std::path::Path,
    child: &mut tokio::process::Child,
    page: &str,
    headers: &BTreeMap<String, String>,
) -> Result<Vec<(Url, BTreeMap<String, String>)>> {
    let portfile = profile.join("DevToolsActivePort");
    let info = loop {
        if let Ok(s) = tokio::fs::read_to_string(&portfile).await {
            break s;
        }
        if let Some(exit) = child.try_wait()? {
            anyhow::bail!("Browser exited early: {exit}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let mut lines = info.lines();
    let port = lines.next().context("Missing debug port")?;
    let path = lines.next().context("Missing debug address")?;
    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}{path}")).await?;
    ws.send(Message::Text(
        json!({"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}})
            .to_string()
            .into(),
    ))
    .await?;
    let target = loop {
        let m = ws.next().await.context("Browser connection closed")??;
        if let Ok(v) = serde_json::from_str::<Value>(m.to_text().unwrap_or(""))
            && v["id"] == 1
        {
            break v["result"]["targetId"]
                .as_str()
                .context("Could not create a browser tab")?
                .to_owned();
        }
    };
    ws.send(Message::Text(json!({"id":2,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}}).to_string().into())).await?;
    let session = loop {
        let m = ws.next().await.context("Browser connection closed")??;
        if let Ok(v) = serde_json::from_str::<Value>(m.to_text().unwrap_or(""))
            && v["id"] == 2
        {
            break v["result"]["sessionId"]
                .as_str()
                .context("Could not attach to the browser tab")?
                .to_owned();
        }
    };
    for (id, method, params) in [
        (3, "Network.enable", json!({})),
        (4, "Network.setExtraHTTPHeaders", json!({"headers":headers})),
        (5, "Page.enable", json!({})),
        (6, "Page.navigate", json!({"url":page})),
    ] {
        ws.send(Message::Text(
            json!({"id":id,"sessionId":session,"method":method,"params":params})
                .to_string()
                .into(),
        ))
        .await?;
    }
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut requests = HashMap::new();
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut next_id = 10;
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _=tokio::time::sleep_until(deadline)=>break,
            _=tick.tick()=>{next_id+=1;ws.send(Message::Text(json!({"id":next_id,"sessionId":session,"method":"Runtime.evaluate","params":{"expression":"document.querySelectorAll('video').forEach(v=>{v.muted=true;v.play().catch(()=>{})})"}}).to_string().into())).await?;},
            m=ws.next()=>{
                let Some(m)=m else {break;};let m=m?;let Ok(v)=serde_json::from_str::<Value>(m.to_text().unwrap_or("")) else {continue;};
                if v["method"]=="Network.requestWillBeSent" {requests.insert(v["params"]["requestId"].as_str().unwrap_or("").to_owned(),v["params"]["request"].clone());}
                if v["method"]=="Network.responseReceived" {
                    let r=&v["params"]["response"];let mime=r["mimeType"].as_str().unwrap_or("").to_lowercase();let raw=r["url"].as_str().unwrap_or("");
                    if (mime.contains("mpegurl")||mime.contains("dash+xml")||raw.contains(".m3u8")||raw.contains(".mpd")) && seen.insert(raw.to_owned())
                        && let Ok(u)=Url::parse(raw) {let mut h=headers.clone();h.entry("referer".into()).or_insert(page.into());
                            if let Some(req)=requests.get(v["params"]["requestId"].as_str().unwrap_or(""))&& let Some(map)=req["headers"].as_object(){for(k,val)in map{if ["cookie","referer","user-agent","authorization","origin"].contains(&k.to_lowercase().as_str())&& let Some(val)=val.as_str(){h.insert(k.to_lowercase(),val.into());}}}
                            if found.is_empty(){deadline=tokio::time::Instant::now()+Duration::from_secs(2);}
                            found.push((u,h)); if found.len()>=4{break;}
                        }
                }
                if !found.is_empty() && v["method"]=="Page.loadEventFired" {break;}
            }
        }
    }
    // Cookie 可能不出现在 requestWillBeSent 中，显式读取当前媒体域的 Cookie。
    ws.send(Message::Text(json!({"id":999,"sessionId":session,"method":"Network.getCookies","params":{"urls":found.iter().map(|(u,_)|u.as_str()).collect::<Vec<_>>()}}).to_string().into())).await?;
    let cookie_result = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(m) = ws.next().await {
            let m = m?;
            if let Ok(v) = serde_json::from_str::<Value>(m.to_text().unwrap_or(""))
                && v["id"] == 999
            {
                return Ok::<Value, anyhow::Error>(v);
            }
        }
        anyhow::bail!("Connection closed during cookie lookup")
    })
    .await;
    if let Ok(Ok(v)) = cookie_result
        && let Some(cookies) = v["result"]["cookies"].as_array()
    {
        for (u, h) in &mut found {
            let values = cookies
                .iter()
                .filter(|c| {
                    let domain = c["domain"].as_str().unwrap_or("").trim_start_matches('.');
                    let host = u.host_str().unwrap_or("");
                    (host == domain || host.ends_with(&format!(".{domain}")))
                        && u.path().starts_with(c["path"].as_str().unwrap_or("/"))
                })
                .filter_map(|c| Some(format!("{}={}", c["name"].as_str()?, c["value"].as_str()?)))
                .collect::<Vec<_>>();
            if !values.is_empty() {
                h.insert("cookie".into(), values.join("; "));
            }
        }
    }
    anyhow::ensure!(
        !found.is_empty(),
        "No media requests detected after loading the page. Login or a playback action may be required."
    );
    Ok(found)
}
