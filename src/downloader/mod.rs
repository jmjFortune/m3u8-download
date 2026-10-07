pub(super) mod output;
pub mod verify;
use crate::{config::Config, model::Task, resolver::Resolved, store::Store};
use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

struct DownloadProcess {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    exited: bool,
}
impl DownloadProcess {
    fn stop(&mut self) {
        if self.exited {
            return;
        }
        if let Some(pid) = self.child.process_id() {
            #[cfg(unix)]
            // portable-pty 在独立 session 中启动子进程；同时终止引擎及其 FFmpeg 子进程。
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            #[cfg(windows)]
            {
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.exited = true;
    }
}
impl Drop for DownloadProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn log_path(config: &Config, id: i64) -> PathBuf {
    config.data.join("logs").join(format!("{id}.log"))
}
pub fn dependency_check(config: &Config) -> Result<()> {
    for (name, exe, arg) in [
        ("Download engine", config.downloader.as_str(), "--version"),
        ("FFmpeg", config.ffmpeg.as_str(), "-version"),
        ("ffprobe", config.ffprobe.as_str(), "-version"),
    ] {
        let result = std::process::Command::new(exe)
            .arg(arg)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .with_context(|| {
                format!("Could not find {name}: {exe}. Use the Docker image with bundled dependencies or set the PC_* paths.")
            })?;
        anyhow::ensure!(result.success(), "Could not run {name}: {exe}");
    }
    Ok(())
}
fn total_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| {
            if e.path().is_dir() {
                total_size(&e.path())
            } else {
                e.metadata().map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}
/// PTY 同时覆盖 Unix 和 Windows ConPTY，避免下载引擎无终端时的进度输出崩溃。
pub async fn download(
    config: Arc<Config>,
    task: Task,
    media: Resolved,
    store: Arc<Store>,
    cancel: CancellationToken,
) -> Result<PathBuf> {
    tokio::task::spawn_blocking(move || {
        let work = config.data.join("tmp").join(task.id.to_string());
        if work.exists() {
            std::fs::remove_dir_all(&work)?;
        }
        std::fs::create_dir_all(&work)?;
        let mut cmd = CommandBuilder::new(&config.downloader);
        cmd.arg(media.url.as_str());
        cmd.args([
            "--save-dir",
            work.to_str()
                .context("Temporary directory path is not valid UTF-8")?,
            "--tmp-dir",
            work.to_str().unwrap(),
            "--save-name",
            "video",
            "--auto-select",
            "--no-log",
            "--del-after-done",
            "--disable-update-check",
            "--write-meta-json",
            "false",
            "--ffmpeg-binary-path",
            &config.ffmpeg,
            "-M",
            "format=mp4",
        ]);
        cmd.args([
            "--thread-count",
            &config.threads.to_string(),
            "--download-retry-count",
            "3",
        ]);
        if let Some(n) = config.preview_segments {
            cmd.args(["--custom-range", &format!("0-{}", n - 1)]);
        }
        for (k, v) in &media.headers {
            cmd.args(["-H", &format!("{k}: {v}")]);
        }
        let pair = native_pty_system().openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut process = DownloadProcess {
            child: pair.slave.spawn_command(cmd)?,
            exited: false,
        };
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        // 不保留 PTY 的输入写端，下载不需要交互。
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path(&config, task.id))?;
        let reader_thread = std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            let mut written = 0;
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                if written < 8 * 1024 * 1024 {
                    let _ = log.write_all(&buffer[..n]);
                    written += n;
                }
            }
        });
        let start = Instant::now();
        let mut last = Instant::now();
        let exit = loop {
            if cancel.is_cancelled() || start.elapsed() > config.timeout() {
                process.stop();
                drop(pair.master);
                let _ = reader_thread.join();
                anyhow::bail!(if cancel.is_cancelled() {
                    "Task cancelled"
                } else {
                    "Download timed out"
                });
            }
            if let Some(status) = process.child.try_wait()? {
                process.exited = true;
                break status;
            }
            if last.elapsed() > Duration::from_secs(1) {
                let bytes = total_size(&work);
                store.update(
                    task.id,
                    "downloading",
                    "Downloading media segments",
                    task.attempt,
                    bytes,
                    None,
                )?;
                last = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(150));
        };
        drop(pair.master);
        let _ = reader_thread.join();
        anyhow::ensure!(
            exit.success(),
            "Download engine failed. Check the task logs."
        );
        let target = work.join("video.mp4");
        anyhow::ensure!(target.is_file(), "Downloader did not produce an MP4 file");
        Ok(target)
    })
    .await?
}
pub fn append_log(config: &Config, id: i64, message: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(config, id))
    {
        let _ = writeln!(f, "\n[PageCatch] {message}");
    }
}
