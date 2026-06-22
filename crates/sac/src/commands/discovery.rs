use super::*;

#[derive(Clone, Debug)]
pub(super) struct CommandSource {
    pub(super) root: PathBuf,
    pub(super) precedence: u8,
}

pub(super) fn discover_command_sources(workspace_dir: Option<&Path>) -> Vec<CommandSource> {
    let mut sources = Vec::new();

    if let Some(workspace_dir) = workspace_dir {
        let root =
            find_project_root(workspace_dir).unwrap_or_else(|| workspace_dir.to_path_buf());

        sources.push(CommandSource {
            root: root.join(".sac").join("commands"),
            precedence: 0,
        });
        sources.push(CommandSource {
            root: root.join(".opencode").join("commands"),
            precedence: 1,
        });
        sources.push(CommandSource {
            root: root.join(".opencode").join("command"),
            precedence: 2,
        });
    }

    if let Some(sac_home) = sac_home_dir() {
        sources.push(CommandSource {
            root: sac_home.join("commands"),
            precedence: 3,
        });
    }

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let config_base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        sources.push(CommandSource {
            root: config_base.join("opencode").join("commands"),
            precedence: 4,
        });
        sources.push(CommandSource {
            root: config_base.join("opencode").join("command"),
            precedence: 5,
        });
    }

    sources.sort_by_key(|source| source.precedence);
    sources
}

pub(super) fn discover_command_files(root: &Path) -> Vec<(String, PathBuf)> {
    if !root.is_dir() {
        return Vec::new();
    }

    let mut found = Vec::new();
    let mut queue = std::collections::VecDeque::from([(root.to_path_buf(), 0usize)]);
    let mut scanned = 0usize;

    while let Some((dir, depth)) = queue.pop_front() {
        if scanned >= MAX_SCAN_DIRS {
            break;
        }
        scanned += 1;

        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();

            if path.is_dir() {
                if depth < MAX_SCAN_DEPTH {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !matches!(
                        name,
                        ".git" | "node_modules" | "target" | ".venv" | "__pycache__"
                    ) {
                        queue.push_back((path, depth + 1));
                    }
                }
                continue;
            }

            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }

            if let Ok(relative) = path.strip_prefix(root) {
                let name = relative
                    .with_extension("")
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/");
                if !name.is_empty() {
                    found.push((name, path));
                }
            }
        }
    }

    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn find_project_root(workspace_dir: &Path) -> Option<PathBuf> {
    let mut current = Some(workspace_dir);
    while let Some(dir) = current {
        let git_marker = dir.join(".git");
        if git_marker.is_dir() || git_marker.is_file() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}
