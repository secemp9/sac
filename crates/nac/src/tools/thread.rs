use std::time::Duration;
use serde_json::Value;
use tokio::process::Command;
use tokio::time::timeout;

use crate::tools::{require_str, ToolResult};

pub async fn execute(args: Value) -> ToolResult {
    let prompt = match require_str(&args, "prompt") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let context = args.get("context").and_then(|v| v.as_str()).unwrap_or("");

    let full_prompt = if context.is_empty() {
        prompt
    } else {
        format!("Context from previous work:\n{}\n\nTask: {}", context, prompt)
    };

    let timeout_secs: u64 = std::env::var("AGENT_THREAD_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);

    let result = timeout(
        Duration::from_secs(timeout_secs),
        Command::new("nac")
            .args(["--single", &full_prompt])
            .output(),
    )
    .await;

    match result {
        Err(_) => ToolResult {
            content: format!("Thread timed out after {}s", timeout_secs),
            is_error: true,
        },
        Ok(Err(e)) => ToolResult {
            content: format!("Failed to spawn thread: {}", e),
            is_error: true,
        },
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            if !stderr.is_empty() {
                for line in stderr.lines() {
                    eprintln!("  [thread] {}", line);
                }
            }

            if !output.status.success() && stdout.trim().is_empty() {
                ToolResult {
                    content: format!("Thread failed (exit {}):\n{}", output.status.code().unwrap_or(-1), stderr),
                    is_error: true,
                }
            } else {
                ToolResult {
                    content: stdout,
                    is_error: false,
                }
            }
        }
    }
}
