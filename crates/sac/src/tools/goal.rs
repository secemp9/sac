use serde_json::{json, Value};

use crate::goal;
use crate::tools::{require_str, ToolResult, ToolRuntime};
use crate::types::{FunctionDef, ToolDefinition};

pub fn get_goal_definition() -> ToolDefinition {
    ToolDefinition {
        def_type: "function".to_string(),
        function: FunctionDef {
            name: "get_goal".to_string(),
            description: "Get the current goal objective, status, and progress. Returns the goal if one is set, or a message indicating no goal exists.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
    }
}

pub fn update_goal_definition() -> ToolDefinition {
    ToolDefinition {
        def_type: "function".to_string(),
        function: FunctionDef {
            name: "update_goal".to_string(),
            description: "Update the current goal status. Use 'complete' when the objective is fully satisfied. Use 'blocked' when you cannot make further progress after multiple attempts.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["complete", "blocked"],
                        "description": "The new goal status"
                    }
                },
                "required": ["status"]
            }),
        },
    }
}

pub async fn execute_get_goal(_args: Value, runtime: &ToolRuntime) -> ToolResult {
    let session_id = match require_session(runtime) {
        Ok(sid) => sid.to_string(),
        Err(result) => return result,
    };
    let store_path = runtime.store_path.clone();

    let result = tokio::task::spawn_blocking(move || {
        goal::load_goal(&store_path, &session_id)
    })
    .await;

    match result {
        Ok(Ok(Some(g))) => {
            let remaining = if g.max_turns > g.turns_completed {
                g.max_turns - g.turns_completed
            } else {
                0
            };
            ToolResult {
                content: format!(
                    "Goal objective: {}\nStatus: {}\nTurns completed: {}/{}\nRemaining turns: {}\nCreated: {}\nUpdated: {}",
                    g.objective, g.status.label(), g.turns_completed, g.max_turns, remaining, g.created_at, g.updated_at
                ),
                is_error: false,
            }
        }
        Ok(Ok(None)) => ToolResult {
            content: "No goal is currently set.".to_string(),
            is_error: false,
        },
        Ok(Err(e)) => ToolResult {
            content: format!("Error loading goal: {}", e),
            is_error: true,
        },
        Err(e) => ToolResult {
            content: format!("Error: {}", e),
            is_error: true,
        },
    }
}

pub async fn execute_update_goal(args: Value, runtime: &ToolRuntime) -> ToolResult {
    let session_id = match require_session(runtime) {
        Ok(sid) => sid.to_string(),
        Err(result) => return result,
    };
    let store_path = runtime.store_path.clone();

    let status_str = match require_str(&args, "status") {
        Ok(s) => s,
        Err(result) => return result,
    };

    let new_status = match goal::GoalStatus::from_str(&status_str) {
        Some(s) if matches!(s, goal::GoalStatus::Complete | goal::GoalStatus::Blocked) => s,
        _ => {
            return ToolResult {
                content: "Error: status must be 'complete' or 'blocked'".to_string(),
                is_error: true,
            }
        }
    };

    let status_label = new_status.label().to_string();
    let result = tokio::task::spawn_blocking(move || {
        let mut g = match goal::load_goal(&store_path, &session_id)? {
            Some(g) => g,
            None => anyhow::bail!("No goal is currently set"),
        };
        g.status = new_status;
        g.updated_at = goal::now_utc();
        goal::save_goal(&store_path, &session_id, &g)?;
        Ok(g)
    })
    .await;

    match result {
        Ok(Ok(g)) => ToolResult {
            content: format!(
                "Goal updated to '{}'. Objective: {}\nTurns completed: {}/{}",
                status_label, g.objective, g.turns_completed, g.max_turns
            ),
            is_error: false,
        },
        Ok(Err(e)) => ToolResult {
            content: format!("Error: {}", e),
            is_error: true,
        },
        Err(e) => ToolResult {
            content: format!("Error: {}", e),
            is_error: true,
        },
    }
}

fn require_session(runtime: &ToolRuntime) -> Result<&str, ToolResult> {
    runtime.session_id.as_deref().ok_or_else(|| ToolResult {
        content: "Error: goal tools require an active session".to_string(),
        is_error: true,
    })
}
