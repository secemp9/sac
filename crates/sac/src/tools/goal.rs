use serde_json::{json, Value};

use crate::goal;
use crate::goal::GoalStatus;
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
            description: "Update the existing goal.\n\
                Use this tool only to mark the goal achieved or genuinely blocked.\n\
                Set status to `complete` only when the objective has actually been achieved and no required work remains.\n\
                Set status to `blocked` only when the same blocking condition has repeated for at least three consecutive goal turns, counting the original/user-triggered turn and any automatic continuations, and the agent cannot make meaningful progress without user input or an external-state change.\n\
                If the user resumes a goal that was previously marked `blocked`, treat the resumed run as a fresh blocked audit. If the same blocking condition then repeats for at least three consecutive resumed goal turns, set status to `blocked` again.\n\
                Once the blocked threshold is satisfied, do not keep reporting that you are still blocked while leaving the goal active; set status to `blocked`.\n\
                Do not use `blocked` merely because the work is hard, slow, uncertain, incomplete, or would benefit from clarification.\n\
                Do not mark a goal complete merely because its budget is nearly exhausted or because you are stopping work.\n\
                You cannot use this tool to pause, resume, budget-limit, or usage-limit a goal; those status changes are controlled by the user or system.\n\
                When marking a budgeted goal achieved with status `complete`, report the final token usage from the tool result to the user.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["complete", "blocked"],
                        "description": "Required. Set to `complete` only when the objective is achieved and no required work remains. Set to `blocked` only after the same blocking condition has recurred for at least three consecutive goal turns and the agent is at an impasse. After a previously blocked goal is resumed, the resumed run starts a fresh blocked audit."
                    }
                },
                "required": ["status"]
            }),
        },
    }
}

pub fn create_goal_definition() -> ToolDefinition {
    ToolDefinition {
        def_type: "function".to_string(),
        function: FunctionDef {
            name: "create_goal".to_string(),
            description: "Create a goal only when explicitly requested by the user or system/developer instructions; do not infer goals from ordinary tasks.\n\
                Set token_budget only when an explicit token budget is requested. Fails if an unfinished goal exists; use update_goal only for status.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "objective": {
                        "type": "string",
                        "description": "Required. The concrete objective to start pursuing. This starts a new active goal when no goal exists or replaces the current goal when it is complete."
                    },
                    "token_budget": {
                        "type": "integer",
                        "description": "Positive token budget for the new goal. Omit unless explicitly requested."
                    }
                },
                "required": ["objective"]
            }),
        },
    }
}

pub async fn execute_create_goal(args: Value, runtime: &ToolRuntime) -> ToolResult {
    let session_id = match require_session(runtime) {
        Ok(sid) => sid.to_string(),
        Err(result) => return result,
    };
    let store_path = runtime.store_path.clone();

    let objective = match require_str(&args, "objective") {
        Ok(s) => s.trim().to_string(),
        Err(result) => return result,
    };

    if objective.is_empty() {
        return ToolResult {
            content: "Error: objective must not be empty".to_string(),
            is_error: true,
        };
    }

    let token_budget: Option<i64> = args
        .get("token_budget")
        .and_then(|v| v.as_i64());

    if let Some(budget) = token_budget {
        if budget <= 0 {
            return ToolResult {
                content: "Error: token_budget must be positive when provided".to_string(),
                is_error: true,
            };
        }
    }

    let result = tokio::task::spawn_blocking(move || {
        // Check for existing unfinished goal
        if let Ok(Some(existing)) = goal::load_goal(&store_path, &session_id) {
            if !existing.status.is_terminal() {
                anyhow::bail!(
                    "An unfinished goal already exists. Complete or clear the current goal before creating a new one."
                );
            }
        }

        let now = goal::now_utc();
        let new_goal = goal::GoalState {
            goal_id: goal::new_goal_id(),
            objective: objective.clone(),
            status: GoalStatus::Active,
            tokens_used: 0,
            time_used_seconds: 0,
            token_budget,
            created_at: now.clone(),
            updated_at: now,
        };
        goal::save_goal(&store_path, &session_id, &new_goal)?;
        Ok(new_goal)
    })
    .await;

    match result {
        Ok(Ok(g)) => {
            let mut lines = vec![
                format!("Goal created successfully."),
                format!("Objective: {}", g.objective),
                format!("Status: {}", g.status.label()),
            ];
            if let Some(budget) = g.token_budget {
                lines.push(format!("Token budget: {}", budget));
            }
            lines.push(format!("Created: {}", g.created_at));
            ToolResult {
                content: lines.join("\n"),
                is_error: false,
            }
        }
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
            let mut lines = vec![
                format!("Goal objective: {}", g.objective),
                format!("Status: {}", g.status.label()),
                format!("Tokens used: {}", g.tokens_used),
                format!("Time used: {}s", g.time_used_seconds),
            ];
            if let Some(budget) = g.token_budget {
                let remaining = (budget - g.tokens_used).max(0);
                lines.push(format!("Token budget: {}", budget));
                lines.push(format!("Tokens remaining: {}", remaining));
            }
            match g.status {
                goal::GoalStatus::UsageLimited => {
                    lines.push("Note: Goal is paused because the session usage limit was exceeded. The user can resume with /goal resume.".to_string());
                }
                goal::GoalStatus::BudgetLimited => {
                    lines.push("Note: Goal is stopped because its token budget has been exhausted. The user must raise the budget and resume.".to_string());
                }
                _ => {}
            }
            lines.push(format!("Created: {}", g.created_at));
            lines.push(format!("Updated: {}", g.updated_at));
            ToolResult {
                content: lines.join("\n"),
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
        Ok(Ok(g)) => {
            let budget_info = match g.token_budget {
                Some(budget) => format!(
                    "\nTokens used: {} of {} budget",
                    g.tokens_used, budget
                ),
                None => format!("\nTokens used: {}", g.tokens_used),
            };
            ToolResult {
                content: format!(
                    "Goal updated to '{}'. Objective: {}\nTime used: {}s{}",
                    status_label, g.objective, g.time_used_seconds, budget_info
                ),
                is_error: false,
            }
        }
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
