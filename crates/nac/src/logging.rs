use std::fs::{create_dir_all, OpenOptions};
use std::path::PathBuf;
use std::process;
use std::sync::OnceLock;

use tracing_subscriber::{fmt, EnvFilter};

static LOG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
const MAX_LOG_FILES: usize = 12;

pub fn init() {
    let log_path = LOG_PATH.get_or_init(resolve_log_path).clone();

    let Some(log_path) = log_path else {
        return;
    };

    let _ = prune_old_logs(&log_path, MAX_LOG_FILES);

    let Ok(file) = open_log_file(&log_path) else {
        return;
    };

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("nac=debug"));
    let _ = file;
    let writer_path = log_path.clone();

    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(move || open_log_file(&writer_path).expect("log file should be openable"))
        .with_ansi(false)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        .without_time()
        .try_init();
}

pub fn current_log_path() -> Option<PathBuf> {
    LOG_PATH.get().cloned().flatten()
}

fn resolve_log_path() -> Option<PathBuf> {
    crate::paths::nac_log_path_for_pid(process::id())
}

fn open_log_file(path: &PathBuf) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn prune_old_logs(current_path: &PathBuf, keep: usize) -> std::io::Result<()> {
    let Some(logs_dir) = current_path.parent() else {
        return Ok(());
    };

    let current_name = current_path.file_name().map(|name| name.to_owned());
    let mut entries = std::fs::read_dir(logs_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("nac-") && name.ends_with(".log"))
        })
        .collect::<Vec<_>>();

    entries.sort_by_key(|entry| {
        entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });

    let mut removable = entries.len().saturating_sub(keep.saturating_sub(1));
    for entry in entries {
        if removable == 0 {
            break;
        }
        if current_name
            .as_ref()
            .is_some_and(|name| *name == entry.file_name())
        {
            continue;
        }
        let _ = std::fs::remove_file(entry.path());
        removable = removable.saturating_sub(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_old_logs_keeps_current_and_newest_files() {
        let root = std::env::temp_dir().join(format!(
            "nac_log_prune_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();

        let names = ["nac-1.log", "nac-2.log", "nac-3.log", "nac-4.log"];
        for name in names {
            std::fs::write(root.join(name), name).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let current = root.join("nac-4.log");
        prune_old_logs(&current, 2).unwrap();

        let mut remaining = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        remaining.sort();

        assert_eq!(remaining, vec!["nac-4.log"]);

        let _ = std::fs::remove_dir_all(root);
    }
}
