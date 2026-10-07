use crate::{
    config::{Config, DownloadSettings},
    downloader::{self, verify},
    model::Task,
    resolver,
    store::Store,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub struct Queue {
    pub config: Arc<Config>,
    pub store: Arc<Store>,
    pub cpu: Arc<crate::cpu::CpuLimit>,
    runtime: Mutex<Runtime>,
    settings_write: Mutex<()>,
    stop: CancellationToken,
}
struct Runtime {
    config: Arc<Config>,
    active: HashMap<i64, CancellationToken>,
}
impl Queue {
    pub fn new(
        config: Arc<Config>,
        store: Arc<Store>,
        cpu: Arc<crate::cpu::CpuLimit>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config: config.clone(),
            store,
            cpu,
            runtime: Mutex::new(Runtime {
                config,
                active: HashMap::new(),
            }),
            settings_write: Mutex::new(()),
            stop: CancellationToken::new(),
        })
    }
    pub fn current_config(&self) -> Arc<Config> {
        self.runtime.lock().unwrap().config.clone()
    }
    pub fn save_settings(&self, settings: DownloadSettings) -> anyhow::Result<DownloadSettings> {
        let _write = self.settings_write.lock().unwrap();
        let config = self.current_config().with_download_settings(&settings)?;
        let settings = config.download_settings();
        let mut runtime = self.runtime.lock().unwrap();
        self.cpu.set(settings.cpu_cores)?;
        if let Err(error) = self.store.save_settings(&settings) {
            self.cpu.set(runtime.config.cpu_cores)?;
            return Err(error);
        }
        runtime.config = Arc::new(config);
        Ok(settings)
    }
    pub fn start(self: &Arc<Self>) {
        if crate::cpu::supported() {
            let q = self.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(1));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut last_error = String::new();
                loop {
                    tokio::select! { _=q.stop.cancelled()=>break, _=interval.tick()=>{} }
                    let cpu = q.cpu.clone();
                    let result = tokio::task::spawn_blocking(move || cpu.reconcile()).await;
                    let error = match result {
                        Ok(Ok(())) => String::new(),
                        Ok(Err(error)) => format!("{error:#}"),
                        Err(error) => error.to_string(),
                    };
                    if !error.is_empty() && error != last_error {
                        eprintln!("CPU core limit: {error}");
                    }
                    last_error = error;
                }
            });
        }
        for _ in 0..8 {
            let q = self.clone();
            tokio::spawn(async move {
                q.worker().await;
            });
        }
    }
    pub fn cancel(&self, id: i64) -> anyhow::Result<bool> {
        let runtime = self.runtime.lock().unwrap();
        let active = &runtime.active;
        let result = self.store.cancel(id)?;
        if let Some(c) = active.get(&id) {
            c.cancel();
        }
        Ok(result)
    }
    pub fn retry(&self, id: i64) -> anyhow::Result<bool> {
        let runtime = self.runtime.lock().unwrap();
        let active = &runtime.active;
        anyhow::ensure!(
            !active.contains_key(&id),
            "Cancellation cleanup is still running. Try again shortly."
        );
        self.store.retry(id)
    }
    pub fn delete(&self, id: i64) -> anyhow::Result<bool> {
        let runtime = self.runtime.lock().unwrap();
        anyhow::ensure!(
            !runtime.active.contains_key(&id),
            "Task is still running or stopping. Cancel it and wait before deleting its record."
        );
        self.store.delete(id)
    }
    pub fn active(&self, id: i64) -> bool {
        self.runtime.lock().unwrap().active.contains_key(&id)
    }
    pub async fn shutdown(&self) {
        self.stop.cancel();
        for token in self.runtime.lock().unwrap().active.values() {
            token.cancel();
        }
        for _ in 0..100 {
            if self.runtime.lock().unwrap().active.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn worker(self: Arc<Self>) {
        loop {
            if self.stop.is_cancelled() {
                break;
            }
            let claimed = {
                let mut runtime = self.runtime.lock().unwrap();
                if runtime.active.len() >= runtime.config.workers as usize {
                    Ok(None)
                } else {
                    let config = runtime.config.clone();
                    self.store.claim().map(|task| {
                        task.map(|task| {
                            let cancel = CancellationToken::new();
                            if self.stop.is_cancelled() {
                                cancel.cancel();
                            }
                            runtime.active.insert(task.id, cancel.clone());
                            (task, cancel, config)
                        })
                    })
                }
            };
            let (task, cancel, config) = match claimed {
                Ok(Some(pair)) => pair,
                Ok(None) => {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    continue;
                }
                Err(e) => {
                    eprintln!("Queue error: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            let id = task.id;
            self.run(task, cancel, config.clone()).await;
            let _ = tokio::fs::remove_dir_all(config.data.join("tmp").join(id.to_string())).await;
            let _ = tokio::fs::remove_file(config.output.join(format!(".{id}.part"))).await;
            self.runtime.lock().unwrap().active.remove(&id);
        }
    }
    async fn run(&self, mut task: Task, cancel: CancellationToken, config: Arc<Config>) {
        let mut last = String::new();
        for attempt in 1..=config.retries {
            if cancel.is_cancelled() {
                return;
            }
            task.attempt = attempt;
            let result = self.attempt(&task, cancel.clone(), config.clone()).await;
            match result {
                Ok(()) => return,
                Err(e) => {
                    last = format!("{e:#}");
                    downloader::append_log(&config, task.id, &last);
                }
            }
            if cancel.is_cancelled() {
                break;
            }
            if attempt < config.retries {
                let _ = self.store.update(
                    task.id,
                    "resolving",
                    &format!("Attempt {attempt} failed. Resolving again shortly: {last}"),
                    attempt,
                    0,
                    None,
                );
                tokio::select! {_=cancel.cancelled()=>return,_=tokio::time::sleep(Duration::from_secs(5*attempt as u64))=>{}}
            }
        }
        if self.stop.is_cancelled() {
            return;
        }
        let _ = self
            .store
            .update(task.id, "fail", &last, task.attempt, 0, None);
    }
    async fn attempt(
        &self,
        task: &Task,
        cancel: CancellationToken,
        config: Arc<Config>,
    ) -> anyhow::Result<()> {
        self.store.update(
            task.id,
            "resolving",
            "Resolving the video page and validating media URLs",
            task.attempt,
            0,
            None,
        )?;
        let media = tokio::select! { _=cancel.cancelled()=>anyhow::bail!("Cancelled"), r=resolver::resolve(&config, &task.url, &task.headers)=>r? };
        let duplicate = tokio::select! { _=cancel.cancelled()=>anyhow::bail!("Cancelled"), r=verify::duplicate(&config, &media)=>match r { Ok(r)=>r, Err(e)=>{downloader::append_log(&config,task.id,&format!("Frame deduplication could not complete; continuing download: {e:#}"));None} } };
        if let Some(path) = duplicate {
            self.store.update(
                task.id,
                "duplicate",
                "Matching frame fingerprints; duplicate video skipped",
                task.attempt,
                0,
                path.to_str(),
            )?;
            return Ok(());
        }
        self.store.update(
            task.id,
            "downloading",
            "Starting download",
            task.attempt,
            0,
            None,
        )?;
        let file = downloader::download(
            config.clone(),
            task.clone(),
            media.clone(),
            self.store.clone(),
            cancel.clone(),
        )
        .await?;
        anyhow::ensure!(!cancel.is_cancelled(), "Cancelled");
        let bytes = tokio::fs::metadata(&file).await?.len();
        self.store.update(
            task.id,
            "verifying",
            "Running strict decoding verification",
            task.attempt,
            bytes,
            None,
        )?;
        let duration = tokio::select! { _=cancel.cancelled()=>anyhow::bail!("Cancelled"), r=verify::verify(&config, &file, media.duration)=>r? };
        anyhow::ensure!(!cancel.is_cancelled(), "Cancelled");
        let suffix = if config.preview_segments.is_some() {
            "_preview"
        } else {
            ""
        };
        let dest = downloader::output::publish(
            &config.output,
            &format!("{}-{}{suffix}.mp4", task.id, task.name),
            &file,
            &cancel,
        )
        .await?;
        self.store.update(
            task.id,
            "ok",
            &format!("Download complete · {duration:.2} s · Strict verification passed"),
            task.attempt,
            bytes,
            dest.to_str(),
        )?;
        Ok(())
    }
}
