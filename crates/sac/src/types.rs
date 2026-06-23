use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

fn serialize_nullable_content<S>(value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(s) => serializer.serialize_str(s),
        None => serializer.serialize_none(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    #[serde(rename = "assistant")]
    Assistant {
        #[serde(serialize_with = "serialize_nullable_content")]
        content: Option<String>,
        #[serde(
            default,
            alias = "reasoning_content",
            skip_serializing_if = "Option::is_none"
        )]
        reasoning_text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_details: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCall>>,
    },
    #[serde(rename = "tool")]
    Tool {
        tool_call_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub def_type: String,
    pub function: FunctionDef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
    /// Cached input tokens (prompt tokens served from cache).
    /// Extracted from `prompt_tokens_details.cached_tokens` (Chat API)
    /// or `input_tokens_details.cached_tokens` (Responses API).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
}

impl Usage {
    /// Accumulate another usage report into this one by summing each field.
    pub fn accumulate(&mut self, other: &Usage) {
        fn add_opt(a: &mut Option<u32>, b: Option<u32>) {
            match (a.as_mut(), b) {
                (Some(existing), Some(val)) => *existing = existing.saturating_add(val),
                (None, Some(val)) => *a = Some(val),
                _ => {}
            }
        }
        add_opt(&mut self.prompt_tokens, other.prompt_tokens);
        add_opt(&mut self.completion_tokens, other.completion_tokens);
        add_opt(&mut self.total_tokens, other.total_tokens);
        add_opt(&mut self.reasoning_tokens, other.reasoning_tokens);
        add_opt(&mut self.cached_tokens, other.cached_tokens);
    }

    /// Compute the goal-accounting token delta following Codex's formula:
    /// `(input_tokens - cached_input_tokens) + output_tokens`.
    ///
    /// Cached input tokens are subtracted because they represent prompt
    /// tokens served from cache and therefore cost less.  Uses `max(0)`
    /// on the subtraction to avoid negative deltas in the unlikely case
    /// that cached tokens exceed prompt tokens (they are normally a
    /// subset).
    pub fn goal_token_delta(&self) -> i64 {
        let prompt = self.prompt_tokens.unwrap_or(0) as i64;
        let cached = self.cached_tokens.unwrap_or(0) as i64;
        let completion = self.completion_tokens.unwrap_or(0) as i64;
        (prompt - cached).max(0).saturating_add(completion)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assistant_content_null() {
        let msg = Message::Assistant {
            content: None,
            reasoning_text: Some("thinking".to_string()),
            reasoning_details: Some(serde_json::json!([{"type": "reasoning"}])),
            tool_calls: Some(vec![ToolCall {
                id: "call_123".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "read".to_string(),
                    arguments: r#"{"path": "src/main.rs"}"#.to_string(),
                },
            }]),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"content\":null"),
            "Expected \"content\":null in JSON but got: {}",
            json
        );
        assert!(
            json.contains("tool_calls"),
            "Expected tool_calls in JSON: {}",
            json
        );
        assert!(
            json.contains("\"reasoning_text\":\"thinking\""),
            "Expected reasoning_text in JSON: {}",
            json
        );
        assert!(
            json.contains("\"reasoning_details\":[{\"type\":\"reasoning\"}]"),
            "Expected reasoning_details in JSON: {}",
            json
        );
    }

    #[test]
    fn test_assistant_reasoning_content_alias_deserializes() {
        let json = r#"{
            "role":"assistant",
            "content":"hello",
            "reasoning_content":"thinking",
            "tool_calls":[]
        }"#;
        let parsed: Message = serde_json::from_str(json).unwrap();
        match parsed {
            Message::Assistant {
                content,
                reasoning_text,
                reasoning_details,
                tool_calls,
            } => {
                assert_eq!(content.as_deref(), Some("hello"));
                assert_eq!(reasoning_text.as_deref(), Some("thinking"));
                assert_eq!(reasoning_details, None);
                assert_eq!(
                    tool_calls.as_ref().map(std::vec::Vec::len),
                    Some(0),
                    "expected empty tool call list"
                );
            }
            other => panic!("expected assistant message, got {:?}", other),
        }
    }

    #[test]
    fn test_message_role_serialization() {
        let system = Message::System {
            content: "hello".to_string(),
        };
        let json = serde_json::to_string(&system).unwrap();
        assert!(json.contains("\"role\":\"system\""), "Got: {}", json);

        let user = Message::User {
            content: "hi".to_string(),
        };
        let json = serde_json::to_string(&user).unwrap();
        assert!(json.contains("\"role\":\"user\""), "Got: {}", json);

        let tool = Message::Tool {
            tool_call_id: "call_abc".to_string(),
            content: "result".to_string(),
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("\"role\":\"tool\""), "Got: {}", json);
        assert!(json.contains("tool_call_id"), "Got: {}", json);
    }

    #[test]
    fn goal_token_delta_without_cached_tokens() {
        let usage = Usage {
            prompt_tokens: Some(100),
            completion_tokens: Some(50),
            total_tokens: Some(150),
            reasoning_tokens: None,
            cached_tokens: None,
        };
        // Without cached tokens, delta = prompt + completion
        assert_eq!(usage.goal_token_delta(), 150);
    }

    #[test]
    fn goal_token_delta_with_cached_tokens() {
        let usage = Usage {
            prompt_tokens: Some(1000),
            completion_tokens: Some(200),
            total_tokens: Some(1200),
            reasoning_tokens: None,
            cached_tokens: Some(800),
        };
        // (1000 - 800) + 200 = 400
        assert_eq!(usage.goal_token_delta(), 400);
    }

    #[test]
    fn goal_token_delta_cached_exceeds_prompt_clamps_to_zero() {
        let usage = Usage {
            prompt_tokens: Some(100),
            completion_tokens: Some(50),
            total_tokens: Some(150),
            reasoning_tokens: None,
            cached_tokens: Some(200),
        };
        // (100 - 200) clamps to 0, then + 50 = 50
        assert_eq!(usage.goal_token_delta(), 50);
    }

    #[test]
    fn goal_token_delta_all_none() {
        let usage = Usage::default();
        assert_eq!(usage.goal_token_delta(), 0);
    }

    #[test]
    fn accumulate_includes_cached_tokens() {
        let mut total = Usage {
            prompt_tokens: Some(100),
            completion_tokens: Some(50),
            total_tokens: Some(150),
            reasoning_tokens: None,
            cached_tokens: Some(80),
        };
        let other = Usage {
            prompt_tokens: Some(200),
            completion_tokens: Some(60),
            total_tokens: Some(260),
            reasoning_tokens: None,
            cached_tokens: Some(150),
        };
        total.accumulate(&other);
        assert_eq!(total.prompt_tokens, Some(300));
        assert_eq!(total.completion_tokens, Some(110));
        assert_eq!(total.total_tokens, Some(410));
        assert_eq!(total.cached_tokens, Some(230));
    }

    #[test]
    fn accumulate_cached_tokens_none_plus_some() {
        let mut total = Usage::default();
        let other = Usage {
            cached_tokens: Some(100),
            ..Default::default()
        };
        total.accumulate(&other);
        assert_eq!(total.cached_tokens, Some(100));
    }
}
