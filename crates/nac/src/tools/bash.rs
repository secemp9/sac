use serde_json::Value;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::process::{isolate_process_group, terminate_child_tree};
use crate::tools::{require_str, ToolResult, ToolRuntime};

const DEFAULT_BASH_TIMEOUT_SECS: u64 = 5 * 60;

pub async fn execute(args: Value, runtime: &ToolRuntime) -> ToolResult {
    let command = match require_str(&args, "command") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let timeout_secs = args
        .get("timeout")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_BASH_TIMEOUT_SECS);

    run_command(
        &command,
        runtime,
        Duration::from_secs(timeout_secs),
        timeout_secs,
    )
    .await
}

async fn run_command(
    command: &str,
    runtime: &ToolRuntime,
    timeout_duration: Duration,
    timeout_secs: u64,
) -> ToolResult {
    if let Some(sandbox) = &runtime.sandbox {
        let args = vec!["-lc".to_string(), command.to_string()];
        return match timeout(timeout_duration, sandbox.exec("bash", &args, None)).await {
            Err(_) => ToolResult {
                content: format!("Command timed out after {}s", timeout_secs),
                is_error: false,
            },
            Ok(Ok(out)) => format_output(out),
            Ok(Err(error)) => ToolResult {
                content: format!("Failed to execute sandboxed command: {}", error),
                is_error: true,
            },
        };
    }

    match run_local_command(command, timeout_duration).await {
        Err(error) => ToolResult {
            content: error,
            is_error: true,
        },
        Ok(LocalCommandOutcome::TimedOut) => ToolResult {
            content: format!("Command timed out after {}s", timeout_secs),
            is_error: false,
        },
        Ok(LocalCommandOutcome::Completed {
            status,
            stdout,
            stderr,
        }) => format_output_parts(status, stdout, stderr),
    }
}

enum LocalCommandOutcome {
    Completed {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    TimedOut,
}

async fn run_local_command(
    command: &str,
    timeout_duration: Duration,
) -> Result<LocalCommandOutcome, String> {
    let mut command_builder = Command::new("bash");
    command_builder
        .arg("-c")
        .arg(command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_process_group(&mut command_builder);

    let mut child = command_builder
        .spawn()
        .map_err(|error| format!("Failed to spawn command: {}", error))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture command stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture command stderr".to_string())?;

    let stdout_handle = tokio::spawn(read_all(stdout));
    let stderr_handle = tokio::spawn(read_all(stderr));

    let status = timeout(timeout_duration, child.wait()).await;
    if status.is_err() {
        terminate_child_tree(&mut child).await;
        let _ = stdout_handle.await;
        let _ = stderr_handle.await;
        return Ok(LocalCommandOutcome::TimedOut);
    }

    let status = status
        .map_err(|_| "Command timed out unexpectedly".to_string())?
        .map_err(|error| format!("Failed to wait for command: {}", error))?;
    let stdout = stdout_handle.await.unwrap_or_default();
    let stderr = stderr_handle.await.unwrap_or_default();

    Ok(LocalCommandOutcome::Completed {
        status,
        stdout,
        stderr,
    })
}

async fn read_all<R>(mut reader: R) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let _ = reader.read_to_end(&mut output).await;
    output
}

fn format_output_parts(status: ExitStatus, stdout: Vec<u8>, stderr: Vec<u8>) -> ToolResult {
    format_output(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn format_output(out: std::process::Output) -> ToolResult {
    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&out.stdout));
    combined.push_str(&String::from_utf8_lossy(&out.stderr));

    let exit_code = out.status.code().unwrap_or(-1);
    let mut content = if exit_code != 0 {
        format!("Exit code: {}\n{}", exit_code, combined)
    } else {
        combined
    };

    if content.len() > 30_000 {
        let temp_path = std::env::temp_dir().join(format!(
            "agent_bash_{}.txt",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        let _ = std::fs::write(&temp_path, &content);
        content.truncate(30_000);
        content.push_str(&format!(
            "\n... (truncated, full output at {})",
            temp_path.display()
        ));
    }

    ToolResult {
        content,
        is_error: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    use crate::events::EventSink;

    fn local_runtime() -> ToolRuntime {
        ToolRuntime {
            store_path: PathBuf::new(),
            session_id: None,
            active_threads: Arc::new(Mutex::new(HashSet::new())),
            event_sink: EventSink::none(),
            sandbox: None,
            mcp: None,
            skills: None,
            activated_skills: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    #[tokio::test]
    async fn test_bash_simple() {
        let result = execute(
            json!({ "command": "echo hello && echo world" }),
            &local_runtime(),
        )
        .await;
        assert!(!result.is_error, "Got error: {}", result.content);
        assert!(result.content.contains("hello"), "Got: {}", result.content);
        assert!(result.content.contains("world"), "Got: {}", result.content);
    }

    #[tokio::test]
    async fn test_bash_timeout() {
        let result = execute(
            json!({ "command": "sleep 300", "timeout": 2 }),
            &local_runtime(),
        )
        .await;
        assert!(!result.is_error);
        assert!(
            result.content.contains("timed out") || result.content.contains("timeout"),
            "Got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_bash_timeout_kills_child_processes() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let marker = std::env::temp_dir().join(format!("nac_bash_timeout_leak_{}", unique));
        let command = format!("(sleep 2; touch {}) & wait", marker.display());

        let result = execute(
            json!({ "command": command, "timeout": 1 }),
            &local_runtime(),
        )
        .await;
        assert!(
            result.content.contains("timed out") || result.content.contains("timeout"),
            "Got: {}",
            result.content
        );

        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            !marker.exists(),
            "timed-out command left a child process running"
        );
    }

    #[tokio::test]
    async fn test_bash_nonzero_exit() {
        let result = execute(json!({ "command": "exit 1" }), &local_runtime()).await;
        assert!(
            !result.is_error,
            "Non-zero exit should not be is_error=true"
        );
        assert!(
            result.content.contains("Exit code:"),
            "Got: {}",
            result.content
        );
    }
}
