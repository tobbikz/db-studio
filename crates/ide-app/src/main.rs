//! db-studio — terminal Database Studio (explorer · SQL · results).

mod app;

use anyhow::Result;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    if let Some(mut dir) = dirs::cache_dir() {
        dir.push("db-studio");
        let _ = std::fs::create_dir_all(&dir);
        let file = std::fs::File::create(dir.join("db-studio.log"))?;
        tracing_subscriber::fmt()
            .with_writer(file)
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .init();
    }

    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    app::run(root).await
}
