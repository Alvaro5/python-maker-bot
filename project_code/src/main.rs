use anyhow::Result;
use dotenvy::dotenv;

mod api;
mod python_exec;
mod interface;
mod utils;


#[tokio::main]
async fn main() -> Result<()> {
    // Charge .env (HF_TOKEN)
    dotenv().ok();

    // Lance ton interface CLI (boucle REPL)
    interface::start_repl().await;

    Ok(())
}
