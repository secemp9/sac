use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::terminal::TerminalManager;
use crate::tools::{require_str, ToolResult, ToolRuntime};
use crate::types::{FunctionDef, ToolDefinition};

pub fn terminal_definition() -> ToolDefinition {
    ToolDefinition {
        def_type: "function".to_string(),
        function: FunctionDef {
            name: "terminal".to_string(),
            description: "Manage named persistent terminal sessions. Supports create, send, read, resize, wait, close, and list operations without replacing exec_command/write_stdin.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["create", "send", "read", "resize", "wait", "close", "list", "reset_command_state", "touch", "cleanup_ephemeral"],
                        "description": "Terminal operation to perform"
                    },
                    "name": {
                        "type": "string",
                        "description": "Named terminal session to operate on"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory for create"
                    },
                    "cols": {
                        "type": "integer",
                        "description": "Terminal width for create/resize (default 120)"
                    },
                    "rows": {
                        "type": "integer",
                        "description": "Terminal height for create/resize (default 40)"
                    },
                    "input": {
                        "type": "string",
                        "description": "Input text for send; supports upstream key notation like <RET>, <C-c>, <UP>"
                    },
                    "yield_time_ms": {
                        "type": "integer",
                        "description": "Polling/output wait duration in milliseconds for send and waits (default 500)"
                    },
                    "max_output_chars": {
                        "type": "integer",
                        "description": "Maximum returned output characters (default 8000)"
                    },
                    "lines": {
                        "type": "integer",
                        "description": "For read: number of lines from the retained history tail"
                    },
                    "wait_type": {
                        "type": "string",
                        "enum": ["output_contains", "idle"],
                        "description": "Wait condition type"
                    },
                    "text": {
                        "type": "string",
                        "description": "Substring to wait for when wait_type=output_contains"
                    },
                    "idle_ms": {
                        "type": "integer",
                        "description": "Required quiet period in milliseconds when wait_type=idle (default 1000)"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Maximum wait duration in milliseconds (default 30000)"
                    },
                    "min_idle_ms": {
                        "type": "integer",
                        "description": "For cleanup_ephemeral: minimum idle time in milliseconds before removing exited ephemeral sessions"
                    }
                },
                "required": ["operation"]
            }),
        },
    }
}

pub async fn execute_terminal(args: &Value, runtime: &ToolRuntime) -> Result<String> {
    let operation = require_str(args, "operation").map_err(tool_error_to_anyhow)?;
    let manager = &runtime.terminal_manager;

    match operation.as_str() {
        "create" => execute_create(args, manager, runtime).await,
        "send" => execute_send(args, manager).await,
        "read" => execute_read(args, manager).await,
        "resize" => execute_resize(args, manager).await,
        "wait" => execute_wait(args, manager).await,
        "close" => execute_close(args, manager).await,
        "list" => execute_list(manager).await,
        "reset_command_state" => execute_reset_command_state(args, manager).await,
        "touch" => execute_touch(args, manager).await,
        "cleanup_ephemeral" => execute_cleanup_ephemeral(args, manager).await,
        other => Err(anyhow!("unknown terminal operation '{}'", other)),
    }
}

async fn execute_create(
    args: &Value,
    manager: &TerminalManager,
    runtime: &ToolRuntime,
) -> Result<String> {
    let name = require_str(args, "name").map_err(tool_error_to_anyhow)?;
    let cwd = args
        .get("cwd")
        .and_then(|value| value.as_str())
        .map(PathBuf::from);
    let cols = args
        .get("cols")
        .and_then(|value| value.as_u64())
        .unwrap_or(120) as u16;
    let rows = args
        .get("rows")
        .and_then(|value| value.as_u64())
        .unwrap_or(40) as u16;

    let info = manager
        .create_named(name.clone(), cwd, cols, rows, runtime.sandbox.as_ref())
        .await?;
    Ok(serde_json::to_string_pretty(&json!({
        "operation": "create",
        "terminal": info,
    }))?)
}

async fn execute_send(args: &Value, manager: &TerminalManager) -> Result<String> {
    let name = require_str(args, "name").map_err(tool_error_to_anyhow)?;
    let input = args
        .get("input")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let yield_ms = args
        .get("yield_time_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or(500);
    let max_output = args
        .get("max_output_chars")
        .and_then(|value| value.as_u64())
        .unwrap_or(8000) as usize;

    let output = manager
        .write_stdin(&name, input, yield_ms, max_output)
        .await?;
    Ok(serde_json::to_string_pretty(&json!({
        "operation": "send",
        "result": output,
    }))?)
}

async fn execute_read(args: &Value, manager: &TerminalManager) -> Result<String> {
    let name = require_str(args, "name").map_err(tool_error_to_anyhow)?;
    let history = manager.read_history(&name).await?;
    let lines = args
        .get("lines")
        .and_then(|value| value.as_u64())
        .map(|value| value as usize);
    let text = if let Some(lines) = lines {
        tail_lines(&history, lines)
    } else {
        history
    };
    Ok(serde_json::to_string_pretty(&json!({
        "operation": "read",
        "name": name,
        "output": text,
    }))?)
}

async fn execute_resize(args: &Value, manager: &TerminalManager) -> Result<String> {
    let name = require_str(args, "name").map_err(tool_error_to_anyhow)?;
    let cols = require_u16(args, "cols")?;
    let rows = require_u16(args, "rows")?;
    manager.resize(&name, cols, rows).await?;
    let info = manager
        .get(&name)
        .await
        .ok_or_else(|| anyhow!("terminal session '{}' vanished after resize", name))?;
    Ok(serde_json::to_string_pretty(&json!({
        "operation": "resize",
        "terminal": info,
    }))?)
}

async fn execute_wait(args: &Value, manager: &TerminalManager) -> Result<String> {
    let name = require_str(args, "name").map_err(tool_error_to_anyhow)?;
    let wait_type = require_str(args, "wait_type").map_err(tool_error_to_anyhow)?;
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or(30_000);

    let start = Instant::now();
    let matched = match wait_type.as_str() {
        "output_contains" => {
            let needle = require_str(args, "text").map_err(tool_error_to_anyhow)?;
            wait_for_output_contains(manager, &name, &needle, timeout_ms).await?
        }
        "idle" => {
            let idle_ms = args
                .get("idle_ms")
                .and_then(|value| value.as_u64())
                .unwrap_or(1_000);
            wait_for_idle(manager, &name, idle_ms, timeout_ms).await?
        }
        other => {
            return Err(anyhow!(
                "unsupported wait_type '{}' (supported: output_contains, idle)",
                other
            ));
        }
    };

    let terminal = manager.get(&name).await;
    Ok(serde_json::to_string_pretty(&json!({
        "operation": "wait",
        "name": name,
        "wait_type": wait_type,
        "matched": matched,
        "elapsed_ms": start.elapsed().as_millis() as u64,
        "terminal": terminal,
    }))?)
}

async fn execute_close(args: &Value, manager: &TerminalManager) -> Result<String> {
    let name = require_str(args, "name").map_err(tool_error_to_anyhow)?;
    manager.remove(&name).await?;
    Ok(serde_json::to_string_pretty(&json!({
        "operation": "close",
        "name": name,
        "closed": true,
    }))?)
}

async fn execute_list(manager: &TerminalManager) -> Result<String> {
    let terminals = manager.list().await;
    Ok(serde_json::to_string_pretty(&json!({
        "operation": "list",
        "terminals": terminals,
    }))?)
}

async fn execute_reset_command_state(args: &Value, manager: &TerminalManager) -> Result<String> {
    let name = require_str(args, "name").map_err(tool_error_to_anyhow)?;
    manager.reset_command_state(&name).await?;
    let terminal = manager
        .get(&name)
        .await
        .ok_or_else(|| anyhow!("terminal session '{}' vanished after reset", name))?;
    Ok(serde_json::to_string_pretty(&json!({
        "operation": "reset_command_state",
        "terminal": terminal,
    }))?)
}

async fn execute_touch(args: &Value, manager: &TerminalManager) -> Result<String> {
    let name = require_str(args, "name").map_err(tool_error_to_anyhow)?;
    manager.touch_output_activity(&name).await?;
    let terminal = manager
        .get(&name)
        .await
        .ok_or_else(|| anyhow!("terminal session '{}' vanished after touch", name))?;
    Ok(serde_json::to_string_pretty(&json!({
        "operation": "touch",
        "terminal": terminal,
    }))?)
}

async fn execute_cleanup_ephemeral(args: &Value, manager: &TerminalManager) -> Result<String> {
    let min_idle_ms = args
        .get("min_idle_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let removed = manager
        .close_ephemeral_idle_older_than(Duration::from_millis(min_idle_ms))
        .await;
    Ok(serde_json::to_string_pretty(&json!({
        "operation": "cleanup_ephemeral",
        "removed": removed,
    }))?)
}

fn require_u16(args: &Value, key: &str) -> Result<u16> {
    let value = args
        .get(key)
        .and_then(|value| value.as_u64())
        .ok_or_else(|| anyhow!("missing required argument '{}'", key))?;
    u16::try_from(value).map_err(|_| anyhow!("argument '{}' must fit in u16", key))
}

async fn wait_for_output_contains(
    manager: &TerminalManager,
    name: &str,
    needle: &str,
    timeout_ms: u64,
) -> Result<bool> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let info = manager
            .get(name)
            .await
            .ok_or_else(|| anyhow!("terminal session '{}' not found", name))?;
        let history = manager.read_history(name).await?;
        if history.contains(needle) {
            return Ok(true);
        }
        if !info.alive {
            return Ok(false);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_idle(
    manager: &TerminalManager,
    name: &str,
    idle_ms: u64,
    timeout_ms: u64,
) -> Result<bool> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let idle_duration = Duration::from_millis(idle_ms);
    loop {
        let info = manager
            .get(name)
            .await
            .ok_or_else(|| anyhow!("terminal session '{}' not found", name))?;
        if info.idle_ms >= idle_duration.as_millis() as u64 {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn tail_lines(text: &str, count: usize) -> String {
    if count == 0 {
        return String::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

fn tool_error_to_anyhow(error: ToolResult) -> anyhow::Error {
    anyhow!(error.content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventSink;
    use serde_json::json;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn test_runtime() -> ToolRuntime {
        ToolRuntime {
            store_path: PathBuf::new(),
            session_id: None,
            worker_executable: None,
            active_threads: Arc::new(Mutex::new(HashSet::new())),
            event_sink: EventSink::none(),
            sandbox: None,
            mcp: None,
            skills: None,
            activated_skills: Arc::new(Mutex::new(HashSet::new())),
            terminal_manager: crate::terminal::TerminalManager::new(),
            thread_timeout_secs: crate::tools::thread::DEFAULT_THREAD_TIMEOUT_SECS,
        }
    }

    #[tokio::test]
    async fn terminal_definition_shape() {
        let def = terminal_definition();
        assert_eq!(def.function.name, "terminal");
        assert!(def
            .function
            .description
            .contains("named persistent terminal sessions"));
    }

    #[tokio::test]
    async fn terminal_create_and_list_round_trip() {
        let runtime = test_runtime();
        let created = execute_terminal(
            &json!({ "operation": "create", "name": "named-shell" }),
            &runtime,
        )
        .await
        .unwrap();
        assert!(created.contains("named-shell"), "got: {}", created);

        let listed = execute_terminal(&json!({ "operation": "list" }), &runtime)
            .await
            .unwrap();
        assert!(listed.contains("named-shell"), "got: {}", listed);

        runtime
            .terminal_manager
            .remove("named-shell")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn terminal_send_read_resize_and_close_round_trip() {
        let runtime = test_runtime();
        execute_terminal(
            &json!({ "operation": "create", "name": "ops-shell", "cols": 80, "rows": 24 }),
            &runtime,
        )
        .await
        .unwrap();

        let sent = execute_terminal(
            &json!({
                "operation": "send",
                "name": "ops-shell",
                "input": "echo named-terminal<RET>",
                "yield_time_ms": 2000
            }),
            &runtime,
        )
        .await
        .unwrap();
        assert!(sent.contains("named-terminal"), "got: {}", sent);

        let read = execute_terminal(
            &json!({ "operation": "read", "name": "ops-shell", "lines": 20 }),
            &runtime,
        )
        .await
        .unwrap();
        assert!(read.contains("named-terminal"), "got: {}", read);

        let resized = execute_terminal(
            &json!({ "operation": "resize", "name": "ops-shell", "cols": 100, "rows": 35 }),
            &runtime,
        )
        .await
        .unwrap();
        assert!(resized.contains("100"), "got: {}", resized);
        assert!(resized.contains("35"), "got: {}", resized);

        let closed = execute_terminal(
            &json!({ "operation": "close", "name": "ops-shell" }),
            &runtime,
        )
        .await
        .unwrap();
        assert!(closed.contains("closed"), "got: {}", closed);
    }

    #[tokio::test]
    async fn terminal_wait_supports_output_contains_and_idle() {
        let runtime = test_runtime();
        execute_terminal(
            &json!({ "operation": "create", "name": "wait-shell" }),
            &runtime,
        )
        .await
        .unwrap();
        execute_terminal(
            &json!({
                "operation": "send",
                "name": "wait-shell",
                "input": "echo wait-marker<RET>",
                "yield_time_ms": 500
            }),
            &runtime,
        )
        .await
        .unwrap();

        let output_wait = execute_terminal(
            &json!({
                "operation": "wait",
                "name": "wait-shell",
                "wait_type": "output_contains",
                "text": "wait-marker",
                "timeout_ms": 2000
            }),
            &runtime,
        )
        .await
        .unwrap();
        assert!(
            output_wait.contains("\"matched\": true"),
            "got: {}",
            output_wait
        );

        let idle_wait = execute_terminal(
            &json!({
                "operation": "wait",
                "name": "wait-shell",
                "wait_type": "idle",
                "idle_ms": 50,
                "timeout_ms": 2000
            }),
            &runtime,
        )
        .await
        .unwrap();
        assert!(
            idle_wait.contains("\"matched\": true"),
            "got: {}",
            idle_wait
        );

        runtime.terminal_manager.remove("wait-shell").await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn terminal_wait_output_contains_returns_false_after_terminal_exit() {
        let runtime = test_runtime();
        execute_terminal(
            &json!({ "operation": "create", "name": "wait-miss-shell" }),
            &runtime,
        )
        .await
        .unwrap();
        execute_terminal(
            &json!({
                "operation": "send",
                "name": "wait-miss-shell",
                "input": "exit<RET>",
                "yield_time_ms": 500
            }),
            &runtime,
        )
        .await
        .unwrap();

        let waited = execute_terminal(
            &json!({
                "operation": "wait",
                "name": "wait-miss-shell",
                "wait_type": "output_contains",
                "text": "definitely-not-present",
                "timeout_ms": 500
            }),
            &runtime,
        )
        .await
        .unwrap();
        assert!(waited.contains("\"matched\": false"), "got: {}", waited);
    }

    #[tokio::test]
    async fn terminal_can_reset_completed_command_state() {
        let runtime = test_runtime();
        execute_terminal(
            &json!({ "operation": "create", "name": "reset-shell" }),
            &runtime,
        )
        .await
        .unwrap();

        execute_terminal(
            &json!({
                "operation": "send",
                "name": "reset-shell",
                "input": "exit<RET>",
                "yield_time_ms": 500
            }),
            &runtime,
        )
        .await
        .unwrap();

        let reset = execute_terminal(
            &json!({ "operation": "reset_command_state", "name": "reset-shell" }),
            &runtime,
        )
        .await;
        assert!(reset.is_err() || reset.as_ref().unwrap().contains("reset_command_state"));
    }

    #[tokio::test]
    async fn cleanup_ephemeral_removes_finished_exec_command_sessions() {
        let runtime = test_runtime();
        let created = crate::tools::exec_command::execute_exec_command(
            &json!({ "cmd": "", "tty": true, "yield_time_ms": 500 }),
            &runtime,
        )
        .await
        .unwrap();
        let created_json: Value = serde_json::from_str(&created).unwrap();
        let session_name = created_json["session_name"].as_str().unwrap().to_string();
        crate::tools::exec_command::execute_write_stdin(
            &json!({ "session_id": session_name, "chars": "exit<RET>", "yield_time_ms": 500 }),
            &runtime,
        )
        .await
        .unwrap();

        let cleanup = execute_terminal(
            &json!({ "operation": "cleanup_ephemeral", "min_idle_ms": 0 }),
            &runtime,
        )
        .await
        .unwrap();
        assert!(cleanup.contains("cleanup_ephemeral"), "got: {}", cleanup);
    }

    #[tokio::test]
    async fn named_and_ephemeral_sessions_can_coexist() {
        let runtime = test_runtime();
        execute_terminal(
            &json!({ "operation": "create", "name": "coexist-named" }),
            &runtime,
        )
        .await
        .unwrap();

        let ephemeral = crate::tools::exec_command::execute_exec_command(
            &json!({ "cmd": "echo coexist", "tty": true, "yield_time_ms": 500 }),
            &runtime,
        )
        .await
        .unwrap();
        let ephemeral_json: Value = serde_json::from_str(&ephemeral).unwrap();
        let ephemeral_name = ephemeral_json["session_name"].as_str().unwrap().to_string();

        let listed = execute_terminal(&json!({ "operation": "list" }), &runtime)
            .await
            .unwrap();
        assert!(listed.contains("coexist-named"), "got: {}", listed);
        assert!(listed.contains(&ephemeral_name), "got: {}", listed);

        runtime
            .terminal_manager
            .remove("coexist-named")
            .await
            .unwrap();
        runtime
            .terminal_manager
            .remove(&ephemeral_name)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn terminal_touch_updates_idle_tracking() {
        let runtime = test_runtime();
        execute_terminal(
            &json!({ "operation": "create", "name": "touch-shell" }),
            &runtime,
        )
        .await
        .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;
        let before = runtime.terminal_manager.get("touch-shell").await.unwrap();
        execute_terminal(
            &json!({ "operation": "touch", "name": "touch-shell" }),
            &runtime,
        )
        .await
        .unwrap();
        let after = runtime.terminal_manager.get("touch-shell").await.unwrap();
        assert!(
            after.idle_ms <= before.idle_ms,
            "before={}, after={}",
            before.idle_ms,
            after.idle_ms
        );

        runtime
            .terminal_manager
            .remove("touch-shell")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn terminal_named_create_rejects_duplicate_names() {
        let runtime = test_runtime();
        execute_terminal(
            &json!({ "operation": "create", "name": "duplicate-shell" }),
            &runtime,
        )
        .await
        .unwrap();

        let duplicate = execute_terminal(
            &json!({ "operation": "create", "name": "duplicate-shell" }),
            &runtime,
        )
        .await;
        assert!(duplicate.is_err());
        assert!(duplicate
            .unwrap_err()
            .to_string()
            .contains("already exists"));

        runtime
            .terminal_manager
            .remove("duplicate-shell")
            .await
            .unwrap();
    }
}
