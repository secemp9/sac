use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use crate::api::OpenAiClient;
use crate::tools::{self, ToolResult, ToolRuntime};
use crate::types::{Message, ToolCall, ToolDefinition};

#[derive(Clone, Copy, Debug)]
pub enum AgentMode {
    Worker,
    Orchestrator,
}

pub struct AgentConfig {
    pub mode: AgentMode,
    pub store_path: PathBuf,
    pub session_id: Option<String>,
    pub initial_messages: Vec<Message>,
}

pub struct Agent {
    client: OpenAiClient,
    max_iterations: usize,
    pub messages: Vec<Message>,
    tool_defs: Vec<ToolDefinition>,
    tool_runtime: ToolRuntime,
}

impl Agent {
    pub fn new(client: OpenAiClient) -> Self {
        Self::default(client)
    }

    pub fn with_config(client: OpenAiClient, config: AgentConfig) -> Self {
        let max_iterations = std::env::var("AGENT_MAX_ITERATIONS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(100);

        let cwd = std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| ".".to_string());

        let (system_prompt, tool_defs) = match config.mode {
            AgentMode::Worker => (
                format!(
                    "You are nac, a coding worker. Working directory: {}.\n\n\
                     Complete the task using your tools. Your final response becomes the retained episode \
                     for this dispatch when you are run by the orchestrator. Preserve file paths, decisions, \
                     current state, outcomes, and open issues. Do not dump raw tool traces unless they matter.",
                    cwd
                ),
                tools::worker_tool_definitions(),
            ),
            AgentMode::Orchestrator => (
                format!(
                    "You are nac, a coding agent orchestrator. Working directory: {}.\n\n\
                     You coordinate work through named persistent threads.\n\
                     - A thread reuses its own retained history across dispatches\n\
                     - Referenced source threads contribute only their latest retained episodes\n\
                     - Threads return episode text documents\n\n\
                     Your tools:\n\
                     - thread(name, action, threads?)\n\
                     - threads()\n\
                     - thread_read(name)\n\
                     - thread_delete(name)\n\
                     - compact(name)\n\n\
                     You must use threads for all coding work. You cannot read, write, or edit files directly.",
                    cwd
                ),
                tools::orchestrator_tool_definitions(),
            ),
        };

        let mut messages = vec![Message::System {
            content: system_prompt,
        }];
        messages.extend(config.initial_messages);

        Self {
            client,
            max_iterations,
            messages,
            tool_defs,
            tool_runtime: ToolRuntime {
                store_path: config.store_path,
                session_id: config.session_id,
                active_threads: Arc::new(Mutex::new(HashSet::new())),
            },
        }
    }

    pub fn default(client: OpenAiClient) -> Self {
        Self::with_config(
            client,
            AgentConfig {
                mode: AgentMode::Worker,
                store_path: crate::store::default_store_path(),
                session_id: None,
                initial_messages: Vec::new(),
            },
        )
    }

    pub async fn send(&mut self, prompt: &str) -> Result<String> {
        self.messages.push(Message::User {
            content: prompt.to_string(),
        });

        for iteration in 0..self.max_iterations {
            let response = self
                .client
                .chat(self.messages.clone(), self.tool_defs.clone())
                .await?;

            let choice = response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("No choices in LLM response"))?;

            if choice.finish_reason.as_deref() == Some("length") {
                return Err(anyhow!("Context window full (finish_reason=length)"));
            }

            let has_tool_calls = choice
                .message
                .tool_calls
                .as_ref()
                .map(|tool_calls| !tool_calls.is_empty())
                .unwrap_or(false);

            self.messages.push(Message::Assistant {
                content: choice.message.content.clone(),
                tool_calls: choice.message.tool_calls.clone(),
            });

            if !has_tool_calls {
                return Ok(choice
                    .message
                    .content
                    .unwrap_or_else(|| "[No response]".to_string()));
            }

            let tool_calls = choice.message.tool_calls.unwrap_or_default();
            eprintln!(
                "[agent] iteration {} — {} tool call(s)",
                iteration + 1,
                tool_calls.len()
            );

            let results =
                execute_tools_parallel(tool_calls, self.tool_runtime.clone(), self.client.clone())
                    .await;
            for (tool_call_id, tool_name, result) in results {
                eprintln!("[result] {} => {}", tool_name, first_line(&result.content));
                self.messages.push(Message::Tool {
                    tool_call_id,
                    content: result.content,
                });
            }
        }

        Err(anyhow!("Max iterations ({}) reached", self.max_iterations))
    }
}

async fn execute_tools_parallel(
    tool_calls: Vec<ToolCall>,
    runtime: ToolRuntime,
    client: OpenAiClient,
) -> Vec<(String, String, ToolResult)> {
    let mut join_set: JoinSet<(usize, String, String, ToolResult)> = JoinSet::new();

    for (index, tool_call) in tool_calls.into_iter().enumerate() {
        let id = tool_call.id;
        let name = tool_call.function.name;
        let args_str = tool_call.function.arguments;
        let runtime = runtime.clone();
        let client = client.clone();
        eprintln!("[tool] {}({})", name, preview(&args_str, 120));

        join_set.spawn(async move {
            let args = match serde_json::from_str::<serde_json::Value>(&args_str) {
                Ok(value) => value,
                Err(error) => {
                    return (
                        index,
                        id,
                        name.clone(),
                        ToolResult {
                            content: format!(
                                "Error: failed to parse tool arguments for '{}': {}",
                                name, error
                            ),
                            is_error: true,
                        },
                    );
                }
            };

            let result = tools::execute_tool(&name, args, &runtime, &client).await;
            (index, id, name, result)
        });
    }

    let mut results = Vec::new();
    while let Some(join_result) = join_set.join_next().await {
        match join_result {
            Ok(result) => results.push(result),
            Err(error) => results.push((
                usize::MAX,
                "unknown".to_string(),
                "unknown".to_string(),
                ToolResult {
                    content: format!("Tool task panicked: {}", error),
                    is_error: true,
                },
            )),
        }
    }

    results.sort_by_key(|(index, ..)| *index);
    results
        .into_iter()
        .map(|(_, tool_call_id, tool_name, result)| (tool_call_id, tool_name, result))
        .collect()
}

fn preview(value: &str, max_len: usize) -> String {
    let sanitized = value.replace('\n', "\\n");
    if sanitized.len() <= max_len {
        sanitized
    } else {
        format!("{}...", &sanitized[..max_len])
    }
}

fn first_line(value: &str) -> &str {
    value.lines().next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_creation() {
        let client = OpenAiClient::new_for_test();
        let agent = Agent::default(client);
        assert!(!agent.messages.is_empty());
        assert!(!agent.tool_defs.is_empty());
    }
}
