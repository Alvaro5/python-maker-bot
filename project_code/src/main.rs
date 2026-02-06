use anyhow::Result;
use dotenvy::dotenv;

mod api;
mod config;
mod python_exec;
mod interface;
mod utils;
mod logger;


#[tokio::main]
async fn main() -> Result<()> {
    // Charge .env (HF_TOKEN)
    dotenv().ok();

    let config = config::AppConfig::load();

    // Lance ton interface CLI (boucle REPL)
    interface::start_repl(&config).await;

    Ok(())
}
