use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

pub const STDERR_EVENT_PREFIX: &str = "__SAC_EVENT__";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    RunStarted {
        thread_name: Option<String>,
        prompt_preview: String,
    },
    ModelCallStarted {
        thread_name: Option<String>,
        iteration: usize,
    },
    ToolCallStarted {
        thread_name: Option<String>,
        call_id: String,
        name: String,
        args_preview: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args_detail: Option<String>,
    },
    ToolCallFinished {
        thread_name: Option<String>,
        call_id: String,
        name: String,
        content_preview: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        is_error: bool,
    },
    ThreadStarted {
        name: String,
        action: String,
        source_threads: Vec<String>,
    },
    ThreadSpawned {
        name: String,
        executable: String,
        cwd: String,
        sandboxed: bool,
    },
    ThreadLog {
        name: String,
        line: String,
    },
    TerminalSnapshot {
        thread_name: Option<String>,
        terminals: Vec<crate::terminal::TerminalInfo>,
    },
    ThreadFinished {
        name: String,
        exit_code: i32,
        timed_out: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_reason: Option<String>,
    },
    AssistantMessage {
        thread_name: Option<String>,
        content: String,
    },
    Error {
        thread_name: Option<String>,
        message: String,
    },
    RunFinished {
        thread_name: Option<String>,
    },
    StreamTextDelta {
        thread_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    StreamComplete {
        thread_name: Option<String>,
    },
}

#[derive(Clone, Default)]
pub struct EventSink {
    channel: Option<UnboundedSender<AgentEvent>>,
    stderr_prefixed: bool,
}

impl EventSink {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn channel(channel: UnboundedSender<AgentEvent>) -> Self {
        Self {
            channel: Some(channel),
            stderr_prefixed: false,
        }
    }

    pub fn stderr_prefixed() -> Self {
        Self {
            channel: None,
            stderr_prefixed: true,
        }
    }

    pub fn worker_stderr() -> Self {
        Self::stderr_prefixed()
    }

    pub fn emit(&self, event: AgentEvent) {
        if self.stderr_prefixed {
            if let Ok(encoded) = serde_json::to_string(&event) {
                eprintln!("{}{}", STDERR_EVENT_PREFIX, encoded);
            }
        }

        if let Some(channel) = &self.channel {
            let _ = channel.send(event);
        }
    }
}

pub fn emit_worker_stderr_event(event: &AgentEvent) {
    if let Ok(encoded) = serde_json::to_string(event) {
        eprintln!("{}{}", STDERR_EVENT_PREFIX, encoded);
    }
}

pub fn decode_stderr_event(line: &str) -> Option<AgentEvent> {
    let encoded = line.strip_prefix(STDERR_EVENT_PREFIX)?;
    serde_json::from_str(encoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_prefixed_event_round_trip() {
        let event = AgentEvent::ThreadStarted {
            name: "impl".to_string(),
            action: "inspect auth".to_string(),
            source_threads: vec!["auth".to_string()],
        };
        let encoded = format!(
            "{}{}",
            STDERR_EVENT_PREFIX,
            serde_json::to_string(&event).unwrap()
        );

        let decoded = decode_stderr_event(&encoded).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn decode_prefixed_thread_spawned_round_trip() {
        let event = AgentEvent::ThreadSpawned {
            name: "impl".to_string(),
            executable: "/home/secemp9/.local/bin/sac".to_string(),
            cwd: "/workspace/project".to_string(),
            sandboxed: false,
        };
        let encoded = format!(
            "{}{}",
            STDERR_EVENT_PREFIX,
            serde_json::to_string(&event).unwrap()
        );

        let decoded = decode_stderr_event(&encoded).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn decode_prefixed_event_ignores_plain_lines() {
        assert!(decode_stderr_event("plain stderr line").is_none());
        assert!(decode_stderr_event("2026-01-01T00:00:00Z DEBUG sac::cli log line").is_none());
    }

    #[test]
    fn worker_stderr_sink_is_prefixed() {
        let sink = EventSink::worker_stderr();
        assert!(sink.stderr_prefixed);
        assert!(sink.channel.is_none());
    }
}
