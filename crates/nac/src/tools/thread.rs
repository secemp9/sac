use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use crate::api::ModelClient;
use crate::events::{decode_stderr_event, AgentEvent};
use crate::process::{isolate_process_group, terminate_child_tree};
use crate::store;
use crate::tools::{require_str, require_string_array, ToolResult, ToolRuntime};
use crate::types::ToolDefinition;

const DEFAULT_THREAD_TIMEOUT_SECS: u64 = 60 * 60;

pub fn dispatch_definition() -> ToolDefinition {
    use serde_json::json;
    def(
        "thread",
        "Dispatch a named worker thread. The worker reuses its own retained history and can pull the latest retained episode from other named threads. Default timeout is 3600 seconds.",
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Thread name. Creates if new, reuses if existing." },
                "action": { "type": "string", "description": "Task for the worker." },
                "threads": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Other thread names whose latest retained episodes should be loaded."
                },
                "timeout": { "type": "integer", "description": "Timeout in seconds for this dispatch (default 3600)." }
            },
            "required": ["name", "action"]
        }),
    )
}

pub fn threads_definition() -> ToolDefinition {
    use serde_json::json;
    def(
        "threads",
        "List active threads in the current orchestrator session.",
        json!({
            "type": "object",
            "properties": {}
        }),
    )
}

pub fn thread_read_definition() -> ToolDefinition {
    use serde_json::json;
    def(
        "thread_read",
        "Read the full retained episode history for one thread.",
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Thread name." }
            },
            "required": ["name"]
        }),
    )
}

pub fn thread_delete_definition() -> ToolDefinition {
    use serde_json::json;
    def(
        "thread_delete",
        "Delete one thread and all its retained episodes.",
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Thread name." }
            },
            "required": ["name"]
        }),
    )
}

pub async fn execute_dispatch(
    args: Value,
    runtime: &ToolRuntime,
    client: &ModelClient,
) -> ToolResult {
    let thread_name = match require_str(&args, "name") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let action = match require_str(&args, "action") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let source_threads = match require_string_array(&args, "threads") {
        Ok(v) => v,
        Err(e) => return e,
    };
    let session_id = match require_session(runtime) {
        Ok(s) => s.to_string(),
        Err(e) => return e,
    };

    if !mark_thread_active(runtime, &thread_name).await {
        return ToolResult {
            content: format!(
                "Thread '{}' is already running; retry after the current dispatch completes.",
                thread_name
            ),
            is_error: true,
        };
    }

    let timeout_secs: u64 = args
        .get("timeout")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            std::env::var("AGENT_THREAD_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(DEFAULT_THREAD_TIMEOUT_SECS);

    runtime.event_sink.emit(AgentEvent::ThreadStarted {
        name: thread_name.clone(),
        action: action.clone(),
        source_threads: source_threads.clone(),
    });

    let result = run_worker(
        runtime,
        client,
        &session_id,
        &thread_name,
        &action,
        &source_threads,
        timeout_secs,
    )
    .await;
    unmark_thread_active(runtime, &thread_name).await;

    match result {
        Err(e) => {
            runtime.event_sink.emit(AgentEvent::Error {
                thread_name: Some(thread_name.clone()),
                message: format!("Failed to spawn thread '{}': {}", thread_name, e),
            });
            ToolResult {
                content: format!("Failed to spawn thread '{}': {}", thread_name, e),
                is_error: true,
            }
        }
        Ok(run) if run.timed_out => {
            runtime.event_sink.emit(AgentEvent::ThreadFinished {
                name: thread_name.clone(),
                exit_code: run.exit_code,
                timed_out: true,
            });
            ToolResult {
                content: format!("Thread '{}' timed out after {}s", thread_name, timeout_secs),
                is_error: true,
            }
        }
        Ok(run) if run.exit_code != 0 => {
            runtime.event_sink.emit(AgentEvent::ThreadFinished {
                name: thread_name.clone(),
                exit_code: run.exit_code,
                timed_out: false,
            });
            let details = if !run.stderr.trim().is_empty() {
                run.stderr.trim().to_string()
            } else if !run.stdout.trim().is_empty() {
                run.stdout.trim().to_string()
            } else {
                "no output".to_string()
            };
            ToolResult {
                content: format!(
                    "Thread '{}' failed (exit {}):\n{}",
                    thread_name, run.exit_code, details
                ),
                is_error: true,
            }
        }
        Ok(run) => {
            runtime.event_sink.emit(AgentEvent::ThreadFinished {
                name: thread_name.clone(),
                exit_code: run.exit_code,
                timed_out: false,
            });
            ToolResult {
                content: run.stdout.trim().to_string(),
                is_error: false,
            }
        }
    }
}

pub async fn execute_threads(runtime: &ToolRuntime) -> ToolResult {
    let session_id = match require_session(runtime) {
        Ok(s) => s.to_string(),
        Err(e) => return e,
    };

    let store_path = runtime.store_path.clone();
    let sid = session_id.clone();
    let threads = match tokio::task::spawn_blocking(move || {
        store::list_threads(&store_path, &sid)
    }).await {
        Ok(Ok(threads)) => threads,
        Ok(Err(error)) => {
            return ToolResult {
                content: format!("Error listing threads: {}", error),
                is_error: true,
            }
        }
        Err(join_error) => {
            return ToolResult {
                content: format!("Internal error listing threads: {}", join_error),
                is_error: true,
            }
        }
    };

    if threads.is_empty() {
        return ToolResult {
            content: "No active threads in this session.".to_string(),
            is_error: false,
        };
    }

    let mut output = String::from("Active threads:");
    for thread in threads {
        output.push_str(&format!(
            "\n- {} | {} episodes | created {} | updated {}",
            thread.name, thread.episode_count, thread.created_at, thread.updated_at
        ));
        if let Some(action) = thread.latest_action.as_deref() {
            output.push_str(&format!(" | last action: {}", action));
        }
    }

    ToolResult {
        content: output,
        is_error: false,
    }
}

pub async fn execute_thread_read(args: Value, runtime: &ToolRuntime) -> ToolResult {
    let thread_name = match require_str(&args, "name") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let session_id = match require_session(runtime) {
        Ok(s) => s.to_string(),
        Err(e) => return e,
    };

    let store_path = runtime.store_path.clone();
    let sid = session_id.clone();
    let tname = thread_name.clone();
    match tokio::task::spawn_blocking(move || {
        store::thread_read(&store_path, &sid, &tname)
    }).await {
        Ok(Ok(episodes)) => ToolResult {
            content: store::render_thread_document(&thread_name, &episodes),
            is_error: false,
        },
        Ok(Err(error)) => ToolResult {
            content: format!("Error reading thread '{}': {}", thread_name, error),
            is_error: true,
        },
        Err(join_error) => ToolResult {
            content: format!("Internal error reading thread '{}': {}", thread_name, join_error),
            is_error: true,
        },
    }
}

pub async fn execute_thread_delete(args: Value, runtime: &ToolRuntime) -> ToolResult {
    let thread_name = match require_str(&args, "name") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let session_id = match require_session(runtime) {
        Ok(s) => s.to_string(),
        Err(e) => return e,
    };

    if is_thread_active(runtime, &thread_name).await {
        return ToolResult {
            content: format!(
                "Thread '{}' is currently running; wait for it to finish before deleting it.",
                thread_name
            ),
            is_error: true,
        };
    }

    let store_path = runtime.store_path.clone();
    let sid = session_id.clone();
    let tname = thread_name.clone();
    match tokio::task::spawn_blocking(move || {
        store::delete_thread(&store_path, &sid, &tname)
    }).await {
        Ok(Ok(true)) => ToolResult {
            content: format!(
                "Deleted thread '{}' and its retained episodes.",
                thread_name
            ),
            is_error: false,
        },
        Ok(Ok(false)) => ToolResult {
            content: format!("Thread '{}' does not exist in this session.", thread_name),
            is_error: true,
        },
        Ok(Err(error)) => ToolResult {
            content: format!("Error deleting thread '{}': {}", thread_name, error),
            is_error: true,
        },
        Err(join_error) => ToolResult {
            content: format!("Internal error deleting thread '{}': {}", thread_name, join_error),
            is_error: true,
        },
    }
}

fn def(name: &str, description: &str, parameters: serde_json::Value) -> ToolDefinition {
    ToolDefinition {
        def_type: "function".to_string(),
        function: crate::types::FunctionDef {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        },
    }
}

fn require_session(runtime: &ToolRuntime) -> Result<&str, ToolResult> {
    runtime.session_id.as_deref().ok_or_else(|| ToolResult {
        content: "Error: thread tools require an active session".to_string(),
        is_error: true,
    })
}

async fn mark_thread_active(runtime: &ToolRuntime, thread_name: &str) -> bool {
    let mut active = runtime.active_threads.lock().await;
    if active.contains(thread_name) {
        false
    } else {
        active.insert(thread_name.to_string());
        true
    }
}

async fn unmark_thread_active(runtime: &ToolRuntime, thread_name: &str) {
    runtime.active_threads.lock().await.remove(thread_name);
}

async fn is_thread_active(runtime: &ToolRuntime, thread_name: &str) -> bool {
    runtime.active_threads.lock().await.contains(thread_name)
}

struct WorkerRun {
    stdout: String,
    stderr: String,
    exit_code: i32,
    timed_out: bool,
}

async fn run_worker(
    runtime: &ToolRuntime,
    client: &ModelClient,
    session_id: &str,
    thread_name: &str,
    action: &str,
    source_threads: &[String],
    timeout_secs: u64,
) -> std::io::Result<WorkerRun> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("--worker")
        .arg("--session-id")
        .arg(session_id)
        .arg("--thread-name")
        .arg(thread_name)
        .arg("--action")
        .arg(action)
        .arg("--api-model")
        .arg(client.model.as_str())
        .arg("--api-base-url")
        .arg(client.base_url())
        .arg("--backend")
        .arg(client.backend().as_str())
        .arg("--store-path")
        .arg(runtime.store_path.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(reasoning_effort) = client.reasoning_effort() {
        command.arg("--effort").arg(reasoning_effort.as_str());
    }

    for source_thread in source_threads {
        command.arg("--source-thread").arg(source_thread);
    }
    if let Some(sandbox) = &runtime.sandbox {
        command.args(sandbox.worker_cli_args());
    }
    isolate_process_group(&mut command);

    let mut child = command.spawn()?;

    let stderr = child.stderr.take().unwrap();
    let event_sink = runtime.event_sink.clone();
    let thread_name_for_logs = thread_name.to_string();
    let stderr_handle = tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut output = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(event) = decode_stderr_event(&line) {
                event_sink.emit(event);
            } else {
                event_sink.emit(AgentEvent::ThreadLog {
                    name: thread_name_for_logs.clone(),
                    line: line.clone(),
                });
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(&line);
            }
        }
        output
    });

    let stdout = child.stdout.take().unwrap();
    let stdout_handle = tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut output = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&line);
        }
        output
    });

    let status = timeout(Duration::from_secs(timeout_secs), child.wait()).await;
    let timed_out = status.is_err();
    if timed_out {
        terminate_child_tree(&mut child).await;
    }

    let stderr = stderr_handle.await.unwrap_or_default();
    let stdout = stdout_handle.await.unwrap_or_default();
    let exit_code = match status {
        Ok(wait_result) => wait_result?.code().unwrap_or(-1),
        Err(_) => -1,
    };

    Ok(WorkerRun {
        stdout,
        stderr,
        exit_code,
        timed_out,
    })
}
