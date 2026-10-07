use crate::{config::Config, resolver::Resolved};
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

async fn run(cmd: &mut Command, limit: Duration) -> Result<std::process::Output> {
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    tokio::time::timeout(limit, cmd.output())
        .await
        .context("Media verification timed out")?
        .context("Could not run the media tool")
}
pub async fn probe(config: &Config, path: &Path) -> Result<f64> {
    let o = run(
        Command::new(&config.ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration:stream=codec_type",
                "-of",
                "json",
            ])
            .arg(path),
        Duration::from_secs(20),
    )
    .await?;
    anyhow::ensure!(o.status.success(), "ffprobe could not read the output file");
    let v: Value = serde_json::from_slice(&o.stdout)?;
    anyhow::ensure!(
        v["streams"]
            .as_array()
            .is_some_and(|s| s.iter().any(|s| s["codec_type"] == "video")),
        "The file has no video track"
    );
    let duration = v["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .context("No valid duration found")?;
    anyhow::ensure!(
        duration.is_finite() && duration > 0.0,
        "Invalid video duration"
    );
    Ok(duration)
}
pub async fn verify(config: &Config, path: &Path, expected: Option<f64>) -> Result<f64> {
    anyhow::ensure!(
        tokio::fs::metadata(path).await?.len() > 1024,
        "The output file is too small"
    );
    let duration = probe(config, path).await?;
    if config.preview_segments.is_none()
        && let Some(expected) = expected
    {
        anyhow::ensure!(
            (duration - expected).abs() <= 3.0_f64.max(expected * 0.01),
            "Output duration {duration:.2}s does not match the source playlist duration {expected:.2}s"
        );
    }
    let o = run(
        Command::new(&config.ffmpeg)
            .args(["-v", "error", "-err_detect", "explode", "-xerror", "-i"])
            .arg(path)
            .args(["-f", "null", "-"]),
        config.timeout(),
    )
    .await?;
    anyhow::ensure!(
        o.status.success() && o.stderr.is_empty(),
        "Strict decoding verification failed: {}",
        String::from_utf8_lossy(&o.stderr)
            .chars()
            .take(600)
            .collect::<String>()
    );
    Ok(duration)
}
async fn hashes(
    config: &Config,
    input: &str,
    headers: Option<&std::collections::BTreeMap<String, String>>,
) -> Result<Vec<String>> {
    let mut cmd = Command::new(&config.ffmpeg);
    cmd.args(["-v", "error"]);
    if let Some(h) = headers {
        let h = h
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect::<String>();
        cmd.args(["-headers", &h, "-allowed_extensions", "ALL"]);
    }
    cmd.args([
        "-i",
        input,
        "-t",
        "6",
        "-an",
        "-vf",
        "fps=25,scale=160:90,format=gray",
        "-f",
        "framemd5",
        "-",
    ]);
    let o = run(&mut cmd, Duration::from_secs(25)).await?;
    anyhow::ensure!(o.status.success(), "Could not extract source frames");
    let values = String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.rsplit(',').next().map(|s| s.trim().to_owned()))
        .collect::<Vec<_>>();
    anyhow::ensure!(values.len() >= 50, "Not enough frames");
    Ok(values)
}
fn same_frames(a: &[String], b: &[String]) -> bool {
    // 避免纯黑/静止开场产生虚假的高一致率。
    if a.iter().collect::<std::collections::HashSet<_>>().len() < 20 {
        return false;
    }
    (-15i32..=15).any(|shift| {
        let pairs = a
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                let j = i as i32 + shift;
                if j < 0 {
                    return None;
                }
                b.get(j as usize).map(|w| v == w)
            })
            .collect::<Vec<_>>();
        pairs.len() >= 50
            && pairs.iter().filter(|v| **v).count() as f64 / pairs.len() as f64 >= 0.95
    })
}
pub async fn duplicate(config: &Config, media: &Resolved) -> Result<Option<PathBuf>> {
    if config.preview_segments.is_some() {
        return Ok(None);
    }
    let Some(duration) = media.duration else {
        return Ok(None);
    };
    let mut candidates = Vec::new();
    for p in super::output::video_files(&config.output).await? {
        if let Ok(d) = probe(config, &p).await
            && (d - duration).abs() <= 3.0
        {
            candidates.push(p);
        }
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    let source = match hashes(config, media.url.as_str(), Some(&media.headers)).await {
        Ok(v) => v,
        Err(e) => return Err(e),
    };
    for path in candidates {
        if let Ok(local) = hashes(config, &path.to_string_lossy(), None).await
            && same_frames(&source, &local)
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matching_requires_motion_and_alignment() {
        let a = (0..150).map(|i| format!("frame-{i}")).collect::<Vec<_>>();
        let b = a[5..].to_vec();
        assert!(same_frames(&a, &b));
        assert!(!same_frames(&a, &vec!["other".into(); 150]));
        assert!(!same_frames(
            &vec!["black".into(); 150],
            &vec!["black".into(); 150]
        ));
    }
}
