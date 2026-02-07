use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    gh_watch::cli::run().await
}
