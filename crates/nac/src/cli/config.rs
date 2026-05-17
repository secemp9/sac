use super::*;

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct NacConfig {
    #[serde(default)]
    pub(super) ui: UiConfig,
    #[serde(default)]
    pub(super) storage: StorageConfig,
    #[serde(default)]
    pub(super) model: ModelConfig,
    #[serde(default)]
    pub(super) sandbox: SandboxConfig,
    #[serde(default)]
    pub(super) worker: WorkerConfig,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct UiConfig {
    pub(super) mode: Option<UiModeConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum UiModeConfig {
    Full,
    Compact,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct StorageConfig {
    pub(super) store_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct ModelConfig {
    pub(super) backend: Option<BackendKind>,
    pub(super) model: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) reasoning_effort: Option<ReasoningEffort>,
    pub(super) api_key_env: Option<String>,
    pub(super) api_key: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct SandboxConfig {
    pub(super) image: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct WorkerConfig {
    pub(super) thread_timeout_secs: Option<u64>,
}

impl NacConfig {
    pub(super) fn load() -> Result<Self> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };
        Self::load_from_path(&path)
    }

    pub(super) fn load_from_path(path: &Path) -> Result<Self> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read config {}", path.display()));
            }
        };
        toml::from_str(&raw).with_context(|| format!("failed to parse config {}", path.display()))
    }
}

pub(super) fn config_path() -> Option<PathBuf> {
    crate::paths::nac_config_path()
}

pub(super) fn sample_config() -> String {
    r#"[ui]
# mode = "compact"

[storage]
# store_path = ".nac/store.db"

[model]
# backend = "openai-responses"
# model = "gpt-5.5"
# base_url = "https://api.openai.com/v1"
# reasoning_effort = "xhigh"
# api_key_env = "OPENAI_API_KEY"
# api_key = "paste-a-static-api-key-here-only-if-you-really-want-config-managed-secrets"

[sandbox]
# image = "python:3.13-bookworm"

[worker]
# thread_timeout_secs = 3600
"#
    .to_string()
}

pub(super) fn config_presence_summary(config: &NacConfig) -> Vec<String> {
    let mut entries = Vec::new();

    if let Some(mode) = config.ui.mode {
        entries.push(format!(
            "ui.mode={}",
            match mode {
                UiModeConfig::Full => "full",
                UiModeConfig::Compact => "compact",
            }
        ));
    }
    if config.storage.store_path.is_some() {
        entries.push("storage.store_path".to_string());
    }
    if let Some(backend) = config.model.backend {
        entries.push(format!("model.backend={}", backend.as_str()));
    }
    if config.model.model.is_some() {
        entries.push("model.model".to_string());
    }
    if config.model.base_url.is_some() {
        entries.push("model.base_url".to_string());
    }
    if let Some(effort) = config.model.reasoning_effort {
        entries.push(format!("model.reasoning_effort={}", effort.as_str()));
    }
    if let Some(env_name) = config
        .model
        .api_key_env
        .as_deref()
        .filter(|name| !name.trim().is_empty())
    {
        entries.push(format!("model.api_key_env={}", env_name));
    }
    if config
        .model
        .api_key
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        entries.push("model.api_key=[set]".to_string());
    }
    if config.sandbox.image.is_some() {
        entries.push("sandbox.image".to_string());
    }
    if config.worker.thread_timeout_secs.is_some() {
        entries.push("worker.thread_timeout_secs".to_string());
    }

    entries
}
pub(super) enum OrchestratorSession {
    Active {
        session_id: String,
        snapshot: SessionSnapshot,
    },
    Picker {
        store_path: PathBuf,
    },
}

impl OrchestratorSession {
    pub(super) fn session_id(&self) -> Option<&str> {
        match self {
            Self::Active { session_id, .. } => Some(session_id),
            Self::Picker { .. } => None,
        }
    }

    pub(super) fn store_path(&self) -> PathBuf {
        match self {
            Self::Active { snapshot, .. } => snapshot.store_path.clone(),
            Self::Picker { store_path } => store_path.clone(),
        }
    }

    pub(super) fn into_snapshot(self) -> Option<SessionSnapshot> {
        match self {
            Self::Active { snapshot, .. } => Some(snapshot),
            Self::Picker { .. } => None,
        }
    }
}

pub(super) struct OrchestratorRunConfig {
    pub(super) agent: Agent,
    pub(super) client: ModelClient,
    pub(super) session: OrchestratorSession,
    pub(super) sandbox_status: String,
    pub(super) agents_md_status: String,
    pub(super) workspace_display: String,
    pub(super) workspace_host_path: Option<PathBuf>,
}

pub(super) struct ManagedWorkerRunConfig {
    pub(super) agent: Agent,
    pub(super) store_path: PathBuf,
    pub(super) session_id: String,
    pub(super) thread_name: String,
    pub(super) action: String,
    pub(super) working_directory: String,
    pub(super) sandbox_status: String,
}

pub(super) enum RunState {
    Orchestrator {
        run_config: OrchestratorRunConfig,
        start_in_session_picker: bool,
        ui_mode: tui::UiMode,
    },
    ManagedWorker(ManagedWorkerRunConfig),
}
