use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[value(rename_all = "kebab-case")]
pub enum BackendKind {
    Auto,
    #[serde(rename = "deepseek-chat")]
    #[value(name = "deepseek-chat")]
    DeepSeekChat,
    FireworksChat,
    #[serde(rename = "openai-responses")]
    #[value(name = "openai-responses")]
    OpenAiResponses,
    #[serde(rename = "chatgpt-codex-responses")]
    #[value(name = "chatgpt-codex-responses")]
    ChatGptCodexResponses,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::DeepSeekChat => "deepseek-chat",
            Self::FireworksChat => "fireworks-chat",
            Self::OpenAiResponses => "openai-responses",
            Self::ChatGptCodexResponses => "chatgpt-codex-responses",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    /// A model-defined effort value that this client does not know yet.
    Custom(String),
}

impl ReasoningEffort {
    pub fn as_str(&self) -> &str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ReasoningEffort {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Self::None),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "" => Err("reasoning effort must not be empty".to_string()),
            other => Ok(Self::Custom(other.to_string())),
        }
    }
}

impl serde::Serialize for ReasoningEffort {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for ReasoningEffort {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningSummary {
    Auto,
    Concise,
    Detailed,
}

impl ReasoningSummary {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::Concise => "concise",
            Self::Detailed => "detailed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningContext {
    CurrentTurn,
    AllTurns,
}

impl ReasoningContext {
    pub fn as_str(&self) -> &str {
        match self {
            Self::CurrentTurn => "current_turn",
            Self::AllTurns => "all_turns",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClientOverrides {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub backend: Option<BackendKind>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub reasoning_summary: Option<ReasoningSummary>,
    pub reasoning_context: Option<ReasoningContext>,
    pub api_key_env: Option<String>,
    pub api_key: Option<String>,
}

pub struct TextCompletion {
    pub content: String,
    pub usage: Usage,
}

#[derive(Debug, Clone)]
pub struct AssistantTurn {
    pub content: Option<String>,
    pub reasoning_text: Option<String>,
    pub reasoning_details: Option<Value>,
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone)]
pub struct ModelTurnResponse {
    pub assistant: AssistantTurn,
    pub finish_reason: Option<String>,
    pub usage: Usage,
}
