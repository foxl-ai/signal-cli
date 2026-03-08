mod cli;
mod commands;
mod output;

use anyhow::Context as _;
use clap::Parser;
use directories::ProjectDirs;
use presage::model::identity::OnNewIdentity;
use presage_store_sqlite::SqliteStore;
use tracing::debug;

use crate::cli::Args;

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::metadata::LevelFilter::INFO.into())
        .from_env_lossy()
        .add_directive("libsignal=error".parse().unwrap());
    tracing_subscriber::fmt::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();
}

fn default_db_path() -> String {
    ProjectDirs::from("org", "whisperfish", "signal-cli")
        .expect("could not determine config directory")
        .config_dir()
        .join("signal.db3")
        .display()
        .to_string()
}

fn ensure_parent_dir_exists(path: &str) -> anyhow::Result<()> {
    let parent = std::path::Path::new(path)
        .parent()
        .context("database path has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    Ok(())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let args = Args::parse();

    let db_path = args.db_path.clone().unwrap_or_else(default_db_path);
    ensure_parent_dir_exists(&db_path)?;

    debug!(db_path, "opening database");
    let store = SqliteStore::open_with_passphrase(
        &db_path,
        args.passphrase.as_deref(),
        OnNewIdentity::Trust,
    )
    .await?;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(commands::run(args.command, store, args.output_format))
        .await
}
