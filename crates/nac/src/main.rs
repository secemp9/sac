use std::process;

#[tokio::main]
async fn main() {
    nac::logging::init();
    tracing::debug!(log_path = ?nac::logging::current_log_path(), "logging initialized");

    if let Err(e) = nac::cli::run().await {
        let is_worker = std::env::args_os().nth(1).as_deref() == Some("__worker".as_ref());
        tracing::error!(error = %e, worker = is_worker, "top-level nac failure");
        if !is_worker {
            eprintln!("Error: {}", e);
        }
        process::exit(1);
    }
}
