#[tokio::main]
async fn main() {
    if let Err(err) = gh_watch::cli::run().await {
        eprintln!("{err:#}");
        std::process::exit(gh_watch::cli::exit_code_for_error(&err));
    }
}
