use anyhow::Result;
use dotenvy::dotenv;

pub mod api;
pub mod config;
pub mod python_exec;
pub mod interface;
pub mod utils;
pub mod logger;

/// Run the application: load `.env`, load config, and start the REPL.
pub async fn run() -> Result<()> {
    // Load environment variables from .env
    dotenv().ok();

    let config = config::AppConfig::load();

    interface::start_repl(&config).await;

    Ok(())
}

// Re-exports for library consumers: common useful types
pub use config::AppConfig;
pub use python_exec::{CodeExecutor, ExecutionMode};

