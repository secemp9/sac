use std::process;

#[tokio::main]
async fn main() {
    let is_worker = std::env::args_os().nth(1).as_deref() == Some("__worker".as_ref());
    if is_worker {
        std::panic::set_hook(Box::new(|panic_info| {
            sac::events::emit_worker_stderr_event(&sac::events::AgentEvent::Error {
                thread_name: None,
                message: format!("worker panic: {panic_info}"),
            });
        }));
    }

    sac::logging::init();
    tracing::debug!(log_path = ?sac::logging::current_log_path(), worker = is_worker, "logging initialized");

    if let Err(e) = sac::cli::run().await {
        tracing::error!(error = %e, worker = is_worker, "top-level sac failure");
        if is_worker {
            sac::events::emit_worker_stderr_event(&sac::events::AgentEvent::Error {
                thread_name: None,
                message: format!("worker failure: {e}"),
            });
        } else {
            eprintln!("Error: {}", e);
        }
        process::exit(1);
    }
}
