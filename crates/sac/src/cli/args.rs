use super::*;

#[derive(Parser)]
#[command(
    name = "sac",
    about = "agent",
    after_help = "Commands:\n  sac resume [SESSION_ID]    Continue a saved session\n  sac config [COMMAND]       Manage sac configuration\n  sac codex-auth [COMMAND]   Manage ChatGPT Codex auth\n  sac upgrade                Reinstall the latest sac release"
)]
pub(super) struct RunCli {
    /// Working directory (default: current directory)
    #[arg(short = 'C', long)]
    pub(super) directory: Option<PathBuf>,

    #[command(flatten)]
    pub(super) store: StoreArgs,

    #[command(flatten)]
    pub(super) ui: UiArgs,

    #[command(flatten)]
    pub(super) model: ModelArgs,

    #[command(flatten)]
    pub(super) sandbox: SandboxArgs,
}

#[derive(Parser)]
#[command(
    name = "sac __worker",
    about = "internal managed worker dispatch",
    hide = true
)]
pub(super) struct ManagedWorkerCli {
    #[command(flatten)]
    pub(super) dispatch: WorkerDispatchArgs,

    #[command(flatten)]
    pub(super) store: StoreArgs,

    #[command(flatten)]
    pub(super) model: ModelArgs,

    #[command(flatten)]
    pub(super) sandbox: SandboxArgs,
}

#[derive(clap::Args)]
pub(super) struct StoreArgs {
    /// Override the SQLite store path (default: .sac/store.db)
    #[arg(long)]
    pub(super) store_path: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct UiArgs {
    /// Use the compact single-column TUI layout
    #[arg(long, conflicts_with = "full")]
    pub(super) compact: bool,

    /// Use the full dashboard TUI layout
    #[arg(long, conflicts_with = "compact")]
    pub(super) full: bool,
}

#[derive(clap::Args, Default, Clone)]
pub(super) struct ModelArgs {
    /// Backend wire shape to use for model requests
    #[arg(long, value_enum)]
    pub(super) backend: Option<BackendKind>,

    /// Reasoning effort to request when supported by the selected backend
    #[arg(long = "effort", value_parser = parse_reasoning_effort_cli)]
    pub(super) reasoning_effort: Option<ReasoningEffort>,

    /// Reasoning summary mode: auto, concise, or detailed
    #[arg(long = "reasoning-summary", value_parser = parse_reasoning_summary_cli)]
    pub(super) reasoning_summary: Option<ReasoningSummary>,

    /// Reasoning context scope: current_turn or all_turns
    #[arg(long = "reasoning-context", value_parser = parse_reasoning_context_cli)]
    pub(super) reasoning_context: Option<ReasoningContext>,

    /// Internal API base URL override used by managed workers and resume
    #[arg(long, hide = true)]
    pub(super) api_base_url: Option<String>,

    /// Internal model override used by managed workers and resume
    #[arg(long, hide = true)]
    pub(super) api_model: Option<String>,
}

#[derive(clap::Args)]
pub(super) struct WorkerDispatchArgs {
    /// Session id for the managed worker dispatch
    #[arg(long)]
    pub(super) session_id: String,

    /// Thread name for the managed worker dispatch
    #[arg(long)]
    pub(super) thread_name: String,

    /// Action for the managed worker dispatch
    #[arg(long)]
    pub(super) action: String,

    /// Source threads whose latest retained episodes should be loaded
    #[arg(long = "source-thread")]
    pub(super) source_threads: Vec<String>,
}

#[derive(clap::Args)]
pub(super) struct SandboxArgs {
    /// Run tool execution inside a session-scoped Podman sandbox
    #[arg(long)]
    pub(super) sandbox: bool,

    /// Disable the implicit current-directory mount into /workspace
    #[arg(long)]
    pub(super) no_mount_cwd: bool,

    /// Additional read-write mount in the form HOST:GUEST
    #[arg(long = "mount")]
    pub(super) mounts: Vec<String>,

    /// Additional read-only mount in the form HOST:GUEST
    #[arg(long = "mount-ro")]
    pub(super) mounts_ro: Vec<String>,

    /// Sandbox image to use when --sandbox is enabled
    #[arg(long)]
    pub(super) sandbox_image: Option<String>,

    /// GPU CDI device to expose to the sandbox (repeatable; use 'all' for all NVIDIA GPUs)
    #[arg(long = "sandbox-gpu")]
    pub(super) sandbox_gpus: Vec<String>,

    /// Sandbox /dev/shm size (default: 0, meaning uncapped by Podman)
    #[arg(long = "sandbox-shm-size")]
    pub(super) sandbox_shm_size: Option<String>,

    /// Internal sandbox session key used to attach worker subprocesses
    #[arg(long, hide = true)]
    pub(super) sandbox_session_key: Option<String>,

    /// Internal sandbox workdir used for worker subprocesses
    #[arg(long, hide = true)]
    pub(super) sandbox_workdir: Option<String>,
}

#[derive(Parser)]
#[command(name = "sac resume", about = "resume saved sac sessions")]
pub(super) struct ResumeCli {
    /// Session id to resume
    pub(super) session_id: Option<String>,

    /// Resume the most recently updated session
    #[arg(long)]
    pub(super) last: bool,

    /// Working directory whose store should be inspected (default: current directory)
    #[arg(short = 'C', long)]
    pub(super) directory: Option<PathBuf>,

    #[command(flatten)]
    pub(super) store: StoreArgs,

    #[command(flatten)]
    pub(super) ui: UiArgs,

    #[command(flatten)]
    pub(super) model: ModelArgs,
}

#[derive(Parser)]
#[command(name = "sac config", about = "manage sac configuration")]
pub(super) struct ConfigCli {
    #[command(subcommand)]
    pub(super) command: Option<ConfigCommand>,
}

#[derive(Subcommand)]
pub(super) enum ConfigCommand {
    /// Create a sample config file if missing
    Init,
    /// Print the config file path
    Path,
    /// Print the current process log file path
    LogPath,
    /// List available sac log files
    Logs,
    /// Print the last lines from the current process log file
    TailLog,
    /// Print a diagnostics bundle for sac runtime state and logs
    Doctor,
    /// Print the current config file contents
    Show,
    /// Reload and validate configuration for this invocation
    Reload,
}

#[derive(Parser)]
#[command(name = "sac codex-auth", about = "manage ChatGPT Codex auth")]
pub(super) struct CodexAuthCli {
    #[command(subcommand)]
    pub(super) command: Option<CodexAuthCommand>,
}

#[derive(Subcommand)]
pub(super) enum CodexAuthCommand {
    /// Sign in with ChatGPT (opens browser; use --headless for device code flow)
    Login {
        /// Use device code flow instead of browser login (for SSH/headless environments)
        #[arg(long)]
        headless: bool,
        /// Read an OpenAI API key from stdin (e.g. echo "sk-..." | sac codex-auth login --with-api-key)
        #[arg(long, conflicts_with_all = ["headless", "with_access_token"])]
        with_api_key: bool,
        /// Read a personal access token from stdin (e.g. echo "at-..." | sac codex-auth login --with-access-token)
        #[arg(long, conflicts_with_all = ["headless", "with_api_key"])]
        with_access_token: bool,
    },
    /// Show stored Codex auth status
    Status,
    /// Remove stored Codex auth
    Logout,
}

#[derive(Parser)]
#[command(name = "sac upgrade", about = "reinstall the latest sac release")]
pub(super) struct UpgradeCli {
    /// Install directory to replace (default: current sac executable directory)
    #[arg(long)]
    pub(super) install_dir: Option<PathBuf>,
}

pub(super) enum ParsedCli {
    Run(RunCli),
    ManagedWorker(ManagedWorkerCli),
    Resume(ResumeCli),
    Config(ConfigCli),
    CodexAuth(CodexAuthCli),
    Upgrade(UpgradeCli),
}

pub(super) fn parse_cli() -> ParsedCli {
    let args: Vec<OsString> = std::env::args_os().collect();
    parse_cli_from(args)
}

pub(super) fn parse_cli_from(args: Vec<OsString>) -> ParsedCli {
    if args
        .get(1)
        .is_some_and(|value| value == OsStr::new("resume"))
    {
        let mut resume_args = Vec::with_capacity(args.len().saturating_sub(1));
        resume_args.push(OsString::from("sac resume"));
        resume_args.extend(args.into_iter().skip(2));
        ParsedCli::Resume(ResumeCli::parse_from(resume_args))
    } else if args
        .get(1)
        .is_some_and(|value| value == OsStr::new("__worker"))
    {
        let mut worker_args = Vec::with_capacity(args.len().saturating_sub(1));
        worker_args.push(OsString::from("sac __worker"));
        worker_args.extend(args.into_iter().skip(2));
        ParsedCli::ManagedWorker(ManagedWorkerCli::parse_from(worker_args))
    } else if args
        .get(1)
        .is_some_and(|value| value == OsStr::new("codex-auth"))
    {
        let mut codex_auth_args = Vec::with_capacity(args.len().saturating_sub(1));
        codex_auth_args.push(OsString::from("sac codex-auth"));
        codex_auth_args.extend(args.into_iter().skip(2));
        ParsedCli::CodexAuth(CodexAuthCli::parse_from(codex_auth_args))
    } else if args
        .get(1)
        .is_some_and(|value| value == OsStr::new("config"))
    {
        let mut config_args = Vec::with_capacity(args.len().saturating_sub(1));
        config_args.push(OsString::from("sac config"));
        config_args.extend(args.into_iter().skip(2));
        ParsedCli::Config(ConfigCli::parse_from(config_args))
    } else if args
        .get(1)
        .is_some_and(|value| value == OsStr::new("upgrade"))
    {
        let mut upgrade_args = Vec::with_capacity(args.len().saturating_sub(1));
        upgrade_args.push(OsString::from("sac upgrade"));
        upgrade_args.extend(args.into_iter().skip(2));
        ParsedCli::Upgrade(UpgradeCli::parse_from(upgrade_args))
    } else {
        ParsedCli::Run(RunCli::parse_from(args))
    }
}

fn parse_reasoning_effort_cli(s: &str) -> Result<ReasoningEffort, String> {
    s.parse()
}

fn parse_reasoning_summary_cli(s: &str) -> Result<ReasoningSummary, String> {
    match s {
        "auto" => Ok(ReasoningSummary::Auto),
        "concise" => Ok(ReasoningSummary::Concise),
        "detailed" => Ok(ReasoningSummary::Detailed),
        other => Err(format!(
            "invalid reasoning summary '{}'; expected: auto, concise, detailed",
            other
        )),
    }
}

fn parse_reasoning_context_cli(s: &str) -> Result<ReasoningContext, String> {
    match s {
        "current_turn" => Ok(ReasoningContext::CurrentTurn),
        "all_turns" => Ok(ReasoningContext::AllTurns),
        other => Err(format!(
            "invalid reasoning context '{}'; expected: current_turn, all_turns",
            other
        )),
    }
}
