use std::sync::Mutex;
use std::sync::MutexGuard;

pub mod agent;
pub mod agents_md;
pub mod cli;
pub mod events;
pub mod life;
pub mod logging;
pub mod mcp;
pub mod model;
pub mod paths;
pub mod process;
pub mod sandbox;
pub mod sessions;
pub mod skills;
pub mod store;
pub mod terminal;
pub mod tools;
pub mod tui;
pub mod types;

pub static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

pub fn test_env_lock() -> MutexGuard<'static, ()> {
    TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
