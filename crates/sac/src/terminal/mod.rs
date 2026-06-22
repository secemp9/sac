mod keyparse;
mod manager;
mod session;

pub use keyparse::parse_keys;
pub use manager::TerminalManager;
pub use session::TerminalSession;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandState {
    Idle,
    Running,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalInfo {
    pub name: String,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    pub alive: bool,
    pub idle_ms: u64,
    pub age_ms: u64,
    pub pid: Option<u32>,
    pub command_state: CommandState,
    pub current_command: Option<String>,
    pub last_exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalOutput {
    pub output: String,
    pub exit_code: Option<i32>,
    pub session_name: Option<String>,
    pub wall_time_ms: u64,
    pub output_truncated: bool,
}
