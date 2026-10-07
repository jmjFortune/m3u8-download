use clap::Parser;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadSettings {
    pub output: PathBuf,
    pub workers: u16,
    pub threads: u16,
    pub retries: u16,
    #[serde(default = "crate::cpu::default_cores")]
    pub cpu_cores: u16,
}

#[derive(Debug, Clone, Parser)]
#[command(
    version,
    about = "PageCatch: resolve video pages and download in batches through the web UI"
)]
pub struct Config {
    #[arg(long, env = "PC_HOST", default_value = "127.0.0.1")]
    pub host: String,
    #[arg(long, env = "PC_PORT", default_value_t = 8787)]
    pub port: u16,
    #[arg(long, env = "PC_DATA", default_value = "data")]
    pub data: PathBuf,
    #[arg(long, env = "PC_OUTPUT", default_value = "downloads")]
    pub output: PathBuf,
    #[arg(long, env = "PC_WORKERS", default_value_t = 2, value_parser = clap::value_parser!(u16).range(1..=8))]
    pub workers: u16,
    #[arg(long, env = "PC_THREADS", default_value_t = 4, value_parser = clap::value_parser!(u16).range(1..=32))]
    pub threads: u16,
    #[arg(long, env = "PC_RETRIES", default_value_t = 3, value_parser = clap::value_parser!(u16).range(1..=10))]
    pub retries: u16,
    #[arg(long, env = "PC_CPU_CORES", default_value_t = crate::cpu::default_cores(), value_parser = clap::value_parser!(u16).range(1..))]
    pub cpu_cores: u16,
    #[arg(long, env = "PC_DOWNLOADER", default_value = "N_m3u8DL-RE")]
    pub downloader: String,
    #[arg(long, env = "PC_FFMPEG", default_value = "ffmpeg")]
    pub ffmpeg: String,
    #[arg(long, env = "PC_FFPROBE", default_value = "ffprobe")]
    pub ffprobe: String,
    #[arg(long, env = "PC_BROWSER")]
    pub browser: Option<PathBuf>,
    #[arg(long, env = "PC_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
    #[arg(long, env = "PC_BROWSER_NO_SANDBOX", default_value_t = false)]
    pub browser_no_sandbox: bool,
    #[arg(long, env = "PC_TIMEOUT", default_value_t = 21600)]
    pub timeout: u64,
    #[arg(long, env = "PC_PREVIEW_SEGMENTS")]
    pub preview_segments: Option<u16>,
    /// Check dependencies without starting the server
    #[arg(long)]
    pub doctor: bool,
}
impl Config {
    pub fn download_settings(&self) -> DownloadSettings {
        DownloadSettings {
            output: self.output.clone(),
            workers: self.workers,
            threads: self.threads,
            retries: self.retries,
            cpu_cores: self.cpu_cores,
        }
    }
    pub fn with_download_settings(&self, settings: &DownloadSettings) -> anyhow::Result<Self> {
        use anyhow::Context;
        use std::io::Write;
        anyhow::ensure!(
            (1..=8).contains(&settings.workers),
            "Concurrent downloads must be between 1 and 8"
        );
        anyhow::ensure!(
            (1..=32).contains(&settings.threads),
            "Segment threads must be between 1 and 32"
        );
        anyhow::ensure!(
            (1..=10).contains(&settings.retries),
            "Maximum attempts must be between 1 and 10"
        );
        anyhow::ensure!(
            settings.output.is_absolute(),
            "Save location must be an absolute path on the server"
        );
        std::fs::create_dir_all(&settings.output).context("Could not create the save directory")?;
        let output = std::fs::canonicalize(&settings.output)
            .context("Could not resolve the save directory")?;
        let mut probe =
            tempfile::NamedTempFile::new_in(&output).context("Save location is not writable")?;
        probe
            .write_all(b"PageCatch write check")
            .context("Save location is not writable")?;
        probe.flush().context("Save location is not writable")?;
        let mut config = self.clone();
        config.output = output;
        config.workers = settings.workers;
        config.threads = settings.threads;
        config.retries = settings.retries;
        config.cpu_cores = settings.cpu_cores;
        Ok(config)
    }
    pub fn prepare(&mut self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.data)?;
        std::fs::create_dir_all(&self.output)?;
        self.downloader = executable_path(&self.downloader)?;
        self.ffmpeg = executable_path(&self.ffmpeg)?;
        self.ffprobe = executable_path(&self.ffprobe)?;
        self.data = std::fs::canonicalize(&self.data)?;
        self.output = std::fs::canonicalize(&self.output)?;
        if self.timeout == 0 {
            anyhow::bail!("PC_TIMEOUT must be greater than 0");
        }
        if self.preview_segments == Some(0) {
            anyhow::bail!("Preview segment count must be greater than 0");
        }
        if self.token.as_ref().is_some_and(|s| s.trim().is_empty()) {
            anyhow::bail!("Access token cannot be empty");
        }
        Ok(())
    }
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout)
    }
}

/// 引擎的 ffmpeg 参数要求完整文件路径，不能直接传 PATH 中的命令名。
fn executable_path(name: &str) -> anyhow::Result<String> {
    let direct = PathBuf::from(name);
    let mut candidates = vec![direct];
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            candidates.push(dir.join(name));
            #[cfg(windows)]
            if !name.to_ascii_lowercase().ends_with(".exe") {
                candidates.push(dir.join(format!("{name}.exe")));
            }
        }
    }
    let path = candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| {
            anyhow::anyhow!("Dependency not found: {name}. Use the Docker image with bundled dependencies or set the full path.")
        })?;
    Ok(std::fs::canonicalize(path)?.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_download_settings_keep_existing_values() {
        let old = r#"{"output":"/downloads","workers":2,"threads":4,"retries":3}"#;
        let settings: DownloadSettings = serde_json::from_str(old).unwrap();
        assert_eq!(settings.cpu_cores, crate::cpu::default_cores());
        assert_eq!(settings.output, PathBuf::from("/downloads"));
        assert_eq!(
            (settings.workers, settings.threads, settings.retries),
            (2, 4, 3)
        );
        let mut chosen = settings;
        chosen.cpu_cores = 1;
        assert_eq!(
            serde_json::from_str::<DownloadSettings>(&serde_json::to_string(&chosen).unwrap())
                .unwrap(),
            chosen
        );
    }
}
