use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    runbook_tui::run().await
}
