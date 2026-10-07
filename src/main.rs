use clap::Parser;
use pagecatch::{api, config::Config, downloader, queue::Queue, store::Store};
use std::sync::Arc;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut config = Config::parse();
    config.prepare()?;
    downloader::dependency_check(&config)?;
    if config.doctor {
        println!(
            "Download engine, FFmpeg and ffprobe are available; browser: {:?}",
            pagecatch::resolver::browser::executable(&config)
        );
        return Ok(());
    }
    std::fs::create_dir_all(config.data.join("logs"))?;
    std::fs::create_dir_all(config.data.join("tmp"))?;
    let store = Arc::new(Store::open(&config.data.join("tasks.sqlite"))?);
    if let Some(settings) = store.load_settings()? {
        config = config.with_download_settings(&settings)?;
    }
    let listener = tokio::net::TcpListener::bind((config.host.as_str(), config.port)).await?;
    println!(
        "PageCatch started: http://{}:{} · Save location: {}",
        config.host,
        config.port,
        config.output.display()
    );
    if config.token.is_none() && config.host != "127.0.0.1" && config.host != "localhost" {
        println!("LAN access is enabled. Set PC_TOKEN to control access.");
    }
    let queue = Queue::new(Arc::new(config), store);
    queue.start();
    let shutdown_queue = queue.clone();
    axum::serve(listener, api::router(queue))
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .unwrap();
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            shutdown_queue.shutdown().await;
        })
        .await?;
    Ok(())
}
