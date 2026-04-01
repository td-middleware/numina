use tracing_subscriber::{EnvFilter, fmt};
use anyhow::Result;

pub fn init_logger(log_level: &str) -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(log_level));

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_thread_ids(true)
        .init();

    Ok(())
}
