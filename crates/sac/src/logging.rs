use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::{fmt, EnvFilter};

static LOG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
const MAX_LOG_FILES: usize = 12;
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const MAX_LOG_AGE_DAYS: u64 = 14;

pub fn init() {
    let log_path = LOG_PATH.get_or_init(resolve_log_path).clone();

    let Some(log_path) = log_path else {
        return;
    };

    let _ = rotate_current_log_if_too_large(&log_path, MAX_LOG_BYTES);
    let _ = prune_old_logs(&log_path, MAX_LOG_FILES);
    let _ = prune_logs_older_than(
        &log_path,
        Duration::from_secs(60 * 60 * 24 * MAX_LOG_AGE_DAYS),
    );

    let Ok(file) = open_log_file(&log_path) else {
        return;
    };

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("sac=debug"));
    let _ = file;
    let writer_path = log_path.clone();

    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(move || open_log_file(&writer_path).expect("log file should be openable"))
        .with_ansi(false)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        .with_timer(UtcTime::rfc_3339())
        .try_init();
}

pub fn current_log_path() -> Option<PathBuf> {
    LOG_PATH.get().cloned().flatten()
}

pub fn append_test_log_line(message: &str) -> std::io::Result<()> {
    let Some(path) = resolve_log_path() else {
        return Ok(());
    };
    let mut file = open_log_file(&path)?;
    writeln!(file, "{message}")
}

pub fn list_log_files() -> std::io::Result<Vec<PathBuf>> {
    let Some(logs_dir) = crate::paths::sac_logs_dir() else {
        return Ok(Vec::new());
    };
    let mut entries = if !logs_dir.exists() {
        Vec::new()
    } else {
        std::fs::read_dir(&logs_dir)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("sac-") && name.ends_with(".log"))
            })
            .map(|entry| entry.path())
            .collect::<Vec<_>>()
    };
    entries.sort();
    Ok(entries)
}

pub fn tail_current_log_lines(limit: usize) -> std::io::Result<Vec<String>> {
    let Some(path) = current_log_path().or_else(resolve_log_path) else {
        return Ok(Vec::new());
    };
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)?;
    let lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    let start = lines.len().saturating_sub(limit);
    Ok(lines[start..].to_vec())
}

pub fn log_file_count() -> std::io::Result<usize> {
    Ok(list_log_files()?.len())
}

fn resolve_log_path() -> Option<PathBuf> {
    crate::paths::sac_log_path_for_pid(process::id())
}

fn open_log_file(path: &PathBuf) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn rotate_current_log_if_too_large(path: &PathBuf, max_bytes: u64) -> std::io::Result<()> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() < max_bytes {
        return Ok(());
    }

    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("sac-log");
    let rotated = path.with_file_name(format!("{}-{}.log", stem, timestamp));
    std::fs::rename(path, rotated)?;
    Ok(())
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
                .is_some_and(|name| name.starts_with("sac-") && name.ends_with(".log"))
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

fn prune_logs_older_than(current_path: &PathBuf, max_age: Duration) -> std::io::Result<()> {
    let Some(logs_dir) = current_path.parent() else {
        return Ok(());
    };
    let current_name = current_path.file_name().map(|name| name.to_owned());
    let cutoff = SystemTime::now()
        .checked_sub(max_age)
        .unwrap_or(SystemTime::UNIX_EPOCH);

    for entry in std::fs::read_dir(logs_dir)?.filter_map(|entry| entry.ok()) {
        if current_name
            .as_ref()
            .is_some_and(|name| *name == entry.file_name())
        {
            continue;
        }
        let is_log = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("sac-") && name.ends_with(".log"));
        if !is_log {
            continue;
        }
        let modified = match entry.metadata().and_then(|meta| meta.modified()) {
            Ok(modified) => modified,
            Err(_) => continue,
        };
        if modified < cutoff {
            let _ = std::fs::remove_file(entry.path());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock;
    use std::ffi::OsString;

    fn restore_env(name: &str, value: Option<OsString>) {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }

    #[test]
    fn prune_old_logs_keeps_current_and_newest_files() {
        let root = std::env::temp_dir().join(format!(
            "sac_log_prune_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();

        let names = ["sac-1.log", "sac-2.log", "sac-3.log", "sac-4.log"];
        for name in names {
            std::fs::write(root.join(name), name).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let current = root.join("sac-4.log");
        prune_old_logs(&current, 2).unwrap();

        let mut remaining = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        remaining.sort();

        assert_eq!(remaining, vec!["sac-4.log"]);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn append_test_log_line_creates_and_writes_to_log_file() {
        let _guard = test_env_lock();
        let original_sac_home = std::env::var_os("SAC_HOME");
        let root = std::env::temp_dir().join(format!(
            "sac_log_smoke_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        unsafe {
            std::env::set_var("SAC_HOME", &root);
        }

        let path = crate::paths::sac_log_path_for_pid(std::process::id()).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        append_test_log_line("smoke-log-entry").unwrap();

        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("smoke-log-entry"));

        restore_env("SAC_HOME", original_sac_home);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn list_log_files_and_tail_current_log_lines_work() {
        let _guard = test_env_lock();
        let original_sac_home = std::env::var_os("SAC_HOME");
        let root = std::env::temp_dir().join(format!(
            "sac_log_list_tail_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("logs")).unwrap();
        unsafe {
            std::env::set_var("SAC_HOME", &root);
        }

        let path = crate::paths::sac_log_path_for_pid(std::process::id()).unwrap();
        std::fs::write(root.join("logs/sac-old.log"), "old\n").unwrap();
        std::fs::write(&path, "line-1\nline-2\nline-3\n").unwrap();

        let logs = list_log_files().unwrap();
        assert_eq!(logs.len(), 2);
        let tail = tail_current_log_lines(2).unwrap();
        assert_eq!(tail, vec!["line-2".to_string(), "line-3".to_string()]);

        restore_env("SAC_HOME", original_sac_home);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rotate_current_log_if_too_large_renames_file() {
        let root = std::env::temp_dir().join(format!(
            "sac_log_rotate_{}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let current = root.join("sac-123.log");
        std::fs::write(&current, vec![b'x'; 32]).unwrap();

        rotate_current_log_if_too_large(&current, 8).unwrap();

        assert!(!current.exists());
        let rotated = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(rotated.len(), 1);
        assert!(rotated[0].starts_with("sac-123-"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn prune_logs_older_than_removes_stale_logs_but_keeps_current() {
        let root = std::env::temp_dir().join(format!(
            "sac_log_age_{}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let current = root.join("sac-100.log");
        let stale = root.join("sac-101.log");
        std::fs::write(&current, "current").unwrap();
        std::fs::write(&stale, "stale").unwrap();

        let old_secs = SystemTime::now()
            .checked_sub(Duration::from_secs(60 * 60 * 24 * 30))
            .unwrap()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        filetime::set_file_mtime(&stale, filetime::FileTime::from_unix_time(old_secs, 0)).unwrap();

        prune_logs_older_than(&current, Duration::from_secs(60 * 60 * 24 * 7)).unwrap();

        assert!(current.exists());
        assert!(!stale.exists());

        let _ = std::fs::remove_dir_all(root);
    }
}
