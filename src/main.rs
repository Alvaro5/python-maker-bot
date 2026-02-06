use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    python_maker_bot::run().await
}
