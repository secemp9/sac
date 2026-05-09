use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};

use crate::process::{isolate_process_group, terminate_child_tree};
use crate::sandbox::SandboxSession;

use super::keyparse::parse_keys;
use super::session::{terminal_env, terminal_env_owned, TerminalSession};
use super::{TerminalInfo, TerminalOutput};

struct ManagedTerminal {
    session: TerminalSession,
    kind: SessionKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SessionKind {
    Ephemeral,
    Named,
}

#[derive(Clone)]
pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, ManagedTerminal>>>,
    max_sessions: usize,
}

impl TerminalManager {
    pub fn new() -> Self {
        TerminalManager {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            max_sessions: 16,
        }
    }

    pub async fn create(
        &self,
        name: String,
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
        sandbox: Option<&SandboxSession>,
    ) -> Result<TerminalInfo> {
        self.create_with_kind(name, cwd, cols, rows, sandbox, SessionKind::Ephemeral)
            .await
    }

    pub async fn create_named(
        &self,
        name: String,
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
        sandbox: Option<&SandboxSession>,
    ) -> Result<TerminalInfo> {
        self.create_with_kind(name, cwd, cols, rows, sandbox, SessionKind::Named)
            .await
    }

    async fn create_with_kind(
        &self,
        name: String,
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
        sandbox: Option<&SandboxSession>,
        kind: SessionKind,
    ) -> Result<TerminalInfo> {
        let session = TerminalSession::spawn(name.clone(), cwd, cols, rows, sandbox)?;
        let info = self.session_info(&name, &session);
        let (old, evicted) = {
            let mut sessions = self.sessions.lock().await;

            if kind == SessionKind::Named && sessions.contains_key(&name) {
                anyhow::bail!("terminal session '{}' already exists", name);
            }

            let old = if kind == SessionKind::Ephemeral {
                sessions.remove(&name).map(|managed| managed.session)
            } else {
                None
            };

            let mut evicted = Vec::new();
            while sessions.len() >= self.max_sessions {
                let oldest_key = sessions
                    .iter()
                    .filter(|(_, managed)| managed.kind == SessionKind::Ephemeral)
                    .min_by_key(|(_, managed)| managed.session.created_at)
                    .map(|(k, _)| k.clone());
                if let Some(key) = oldest_key {
                    if let Some(managed) = sessions.remove(&key) {
                        evicted.push(managed.session);
                    }
                } else {
                    break;
                }
            }

            if sessions.len() >= self.max_sessions {
                anyhow::bail!(
                    "terminal session limit reached; no ephemeral session available for eviction"
                );
            }

            sessions.insert(name, ManagedTerminal { session, kind });
            (old, evicted)
        };

        if let Some(mut old) = old {
            let _ = old.kill().await;
        }
        for mut s in evicted {
            let _ = s.kill().await;
        }

        Ok(info)
    }

    pub async fn write_stdin(
        &self,
        name: &str,
        input: &str,
        yield_ms: u64,
        max_output: usize,
    ) -> Result<TerminalOutput> {
        let start = Instant::now();
        let bytes = parse_keys(input);

        {
            let mut sessions = self.sessions.lock().await;
            let session = sessions
                .get_mut(name)
                .with_context(|| format!("terminal session '{}' not found", name))?;
            session.session.refresh_status();
            if !session.session.is_alive() && !bytes.is_empty() {
                return Err(anyhow!("terminal session '{}' has already exited", name));
            }
            if !bytes.is_empty() {
                session.session.write(&bytes)?;
            }
        }

        if !bytes.is_empty() {
            sleep(Duration::from_millis(50)).await;
        }

        let output = self.collect_output(name, yield_ms, start).await?;

        if !bytes.is_empty() {
            sleep(Duration::from_millis(50)).await;
        }

        let ended_session = {
            let mut sessions = self.sessions.lock().await;
            if let Some(session) = sessions.get_mut(name) {
                session.session.refresh_status();
                if session.session.is_alive() {
                    None
                } else {
                    sessions.remove(name).map(|managed| managed.session)
                }
            } else {
                None
            }
        };

        let (session_name, exit_code) = if let Some(mut session) = ended_session {
            (
                None,
                session
                    .wait_for_exit_code()
                    .await
                    .or_else(|| session.exit_code()),
            )
        } else {
            (Some(name.to_string()), None)
        };

        let (output_text, truncated) = head_tail_truncate(&output, max_output);
        Ok(TerminalOutput {
            output: output_text,
            exit_code,
            session_name,
            wall_time_ms: start.elapsed().as_millis() as u64,
            output_truncated: truncated,
        })
    }

    pub async fn exec_one_shot(
        &self,
        cmd: &str,
        cwd: Option<PathBuf>,
        _cols: u16,
        _rows: u16,
        yield_ms: u64,
        max_output: usize,
        sandbox: Option<&SandboxSession>,
    ) -> Result<TerminalOutput> {
        let start = Instant::now();
        let outcome = run_pipe_command(cmd, cwd, Duration::from_millis(yield_ms), sandbox).await?;
        let (exit_code, combined) = match outcome {
            PipeCommandOutcome::Completed(output) => {
                let mut combined = String::new();
                combined.push_str(&String::from_utf8_lossy(&output.stdout));
                combined.push_str(&String::from_utf8_lossy(&output.stderr));
                (Some(output.status.code().unwrap_or(-1)), combined)
            }
            PipeCommandOutcome::TimedOut { stdout, stderr } => {
                let mut combined = format!("Command timed out after {yield_ms}ms\n");
                combined.push_str(&String::from_utf8_lossy(&stdout));
                combined.push_str(&String::from_utf8_lossy(&stderr));
                (None, combined)
            }
        };

        let (output_text, truncated) = head_tail_truncate(&combined, max_output);
        Ok(TerminalOutput {
            output: output_text,
            exit_code,
            session_name: None,
            wall_time_ms: start.elapsed().as_millis() as u64,
            output_truncated: truncated,
        })
    }

    pub async fn remove(&self, name: &str) -> Result<()> {
        let session = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(name).map(|managed| managed.session)
        };
        if let Some(mut session) = session {
            session.kill().await?;
        }
        Ok(())
    }

    pub async fn remove_all(&self) {
        let sessions: Vec<TerminalSession> = {
            let mut sessions = self.sessions.lock().await;
            sessions
                .drain()
                .map(|(_, managed)| managed.session)
                .collect()
        };
        for mut session in sessions {
            let _ = session.kill().await;
        }
    }

    pub async fn list(&self) -> Vec<TerminalInfo> {
        let mut sessions = self.sessions.lock().await;
        sessions
            .iter_mut()
            .map(|(name, managed)| {
                managed.session.refresh_status();
                self.session_info(name, &managed.session)
            })
            .collect()
    }

    pub async fn get(&self, name: &str) -> Option<TerminalInfo> {
        let mut sessions = self.sessions.lock().await;
        sessions.get_mut(name).map(|managed| {
            managed.session.refresh_status();
            self.session_info(&managed.session.name, &managed.session)
        })
    }

    pub async fn contains(&self, name: &str) -> bool {
        let sessions = self.sessions.lock().await;
        sessions.contains_key(name)
    }

    pub async fn resize(&self, name: &str, cols: u16, rows: u16) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(name)
            .with_context(|| format!("terminal session '{}' not found", name))?;
        session.session.resize(cols, rows)
    }

    pub async fn read_history(&self, name: &str) -> Result<String> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(name)
            .with_context(|| format!("terminal session '{}' not found", name))?;
        Ok(session.session.read_history())
    }

    pub async fn reset_command_state(&self, name: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(name)
            .with_context(|| format!("terminal session '{}' not found", name))?;
        session.session.reset_command_state();
        Ok(())
    }

    pub async fn touch_output_activity(&self, name: &str) -> Result<()> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(name)
            .with_context(|| format!("terminal session '{}' not found", name))?;
        session.session.touch_output_activity();
        Ok(())
    }

    pub async fn close_ephemeral_idle_older_than(&self, idle: Duration) -> Vec<String> {
        let removable = {
            let mut sessions = self.sessions.lock().await;
            sessions
                .iter_mut()
                .filter_map(|(name, managed)| {
                    managed.session.refresh_status();
                    if managed.kind == SessionKind::Ephemeral
                        && !managed.session.is_alive()
                        && managed.session.idle_duration() >= idle
                    {
                        Some(name.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };

        for name in &removable {
            let _ = self.remove(name).await;
        }

        removable
    }

    fn session_info(&self, name: &str, session: &TerminalSession) -> TerminalInfo {
        TerminalInfo {
            name: name.to_string(),
            cwd: session.cwd.clone(),
            cols: session.cols,
            rows: session.rows,
            alive: session.is_alive(),
            idle_ms: session.idle_duration().as_millis() as u64,
            age_ms: session.age().as_millis() as u64,
            pid: session.pid(),
            command_state: session.command_state(),
            current_command: session.current_command(),
            last_exit_code: session.last_command_exit_code(),
        }
    }

    async fn collect_output(&self, name: &str, yield_ms: u64, start: Instant) -> Result<String> {
        let deadline = start + Duration::from_millis(yield_ms);
        let mut output = String::new();

        let notify = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(name)
                .ok_or_else(|| anyhow!("session vanished"))?
                .session
                .output_notify()
                .clone()
        };

        loop {
            let (current, alive) = {
                let mut sessions = self.sessions.lock().await;
                let session = sessions
                    .get_mut(name)
                    .ok_or_else(|| anyhow!("session vanished"))?;
                session.session.refresh_status();
                let current = session.session.read_output();
                let alive = session.session.is_alive();
                (current, alive)
            };

            if !current.is_empty() {
                output.push_str(&current);
                if Instant::now() >= deadline {
                    return Ok(output);
                }
                tokio::task::yield_now().await;
                continue;
            }

            if !alive {
                return Ok(output);
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining == Duration::ZERO {
                return Ok(output);
            }

            tokio::select! {
                _ = notify.notified() => continue,
                _ = sleep(remaining) => return Ok(output),
            }
        }
    }
}

async fn run_pipe_command(
    cmd: &str,
    cwd: Option<PathBuf>,
    timeout_duration: Duration,
    sandbox: Option<&SandboxSession>,
) -> Result<PipeCommandOutcome> {
    let mut sandbox_pidfile: Option<String> = None;
    let mut command = if let Some(sb) = sandbox {
        let envs = terminal_env_owned();
        let (mut command, pidfile) = sb.terminal_pipe_command(cmd, cwd.as_deref(), &envs);
        sandbox_pidfile = Some(pidfile);
        isolate_process_group(&mut command);
        command
    } else {
        let mut command = Command::new("bash");
        command.arg("-c").arg(cmd);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        for (key, value) in terminal_env() {
            command.env(key, value);
        }
        isolate_process_group(&mut command);
        command
    };

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().context("failed to spawn command")?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to capture command stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("failed to capture command stderr"))?;

    let stdout_handle = tokio::spawn(read_all(stdout));
    let stderr_handle = tokio::spawn(read_all(stderr));

    let status = match timeout(timeout_duration, child.wait()).await {
        Ok(status) => status.context("failed to wait for command")?,
        Err(_) => {
            if let (Some(sb), Some(pidfile)) = (sandbox, sandbox_pidfile.as_deref()) {
                let _ = sb.terminal_pipe_kill(pidfile).await;
            }
            terminate_child_tree(&mut child).await;
            return Ok(PipeCommandOutcome::TimedOut {
                stdout: stdout_handle.await.unwrap_or_default(),
                stderr: stderr_handle.await.unwrap_or_default(),
            });
        }
    };
    Ok(PipeCommandOutcome::Completed(Output {
        status,
        stdout: stdout_handle.await.unwrap_or_default(),
        stderr: stderr_handle.await.unwrap_or_default(),
    }))
}

enum PipeCommandOutcome {
    Completed(Output),
    TimedOut { stdout: Vec<u8>, stderr: Vec<u8> },
}

async fn read_all<R>(mut reader: R) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let _ = reader.read_to_end(&mut output).await;
    output
}

fn head_tail_truncate(text: &str, max_chars: usize) -> (String, bool) {
    if text.len() <= max_chars {
        return (text.to_string(), false);
    }
    if max_chars == 0 {
        return (String::new(), true);
    }

    let half = max_chars / 2;
    let head = if let Some(idx) = text.char_indices().nth(half).map(|(i, _)| i) {
        &text[..idx]
    } else {
        text
    };
    let tail_start = if let Some(idx) = text
        .char_indices()
        .nth_back(half.saturating_sub(1))
        .map(|(i, _)| i)
    {
        idx
    } else {
        text.len()
    };
    let truncated = format!(
        "{}...\n...[{} chars truncated]...\n{}",
        head,
        text.len().saturating_sub(max_chars),
        &text[tail_start..]
    );
    (truncated, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::{
        SandboxSession, SandboxSpec, DEFAULT_SANDBOX_IMAGE, DEFAULT_SANDBOX_WORKDIR,
    };

    #[test]
    fn terminal_pipe_command_delegates_to_sandbox_session() {
        let sandbox = SandboxSession::new_for_test(SandboxSpec {
            image: DEFAULT_SANDBOX_IMAGE.to_string(),
            mounts: Vec::new(),
            workdir: DEFAULT_SANDBOX_WORKDIR.into(),
            gpu_devices: Vec::new(),
            shm_size: None,
        });

        let envs = terminal_env_owned();
        let (command, pidfile) = sandbox.terminal_pipe_command("echo hello", None, &envs);

        assert!(pidfile.starts_with("/tmp/nac-exec-"));
        assert!(pidfile.ends_with(".pid"));

        let debug = format!("{command:?}");
        assert!(debug.contains("podman"), "expected podman command: {debug}");
        assert!(debug.contains("exec"), "expected exec subcommand: {debug}");
        assert!(debug.contains("TERM=dumb"), "expected TERM=dumb: {debug}");
    }

    #[tokio::test]
    async fn terminal_info_includes_richer_metadata() {
        let manager = TerminalManager::new();
        manager
            .create("info-test".to_string(), None, 90, 30, None)
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let info = manager
            .get("info-test")
            .await
            .expect("missing terminal info");
        assert_eq!(info.name, "info-test");
        assert_eq!(info.cols, 90);
        assert_eq!(info.rows, 30);
        assert!(info.age_ms <= 5_000, "unexpected age_ms: {}", info.age_ms);
        assert!(matches!(
            info.command_state,
            crate::terminal::CommandState::Idle
        ));
        assert!(info.current_command.is_none());
        assert!(info.last_exit_code.is_none());

        manager.remove("info-test").await.unwrap();
    }

    #[tokio::test]
    async fn read_history_retains_terminal_output_after_polling() {
        let manager = TerminalManager::new();
        manager
            .create("history-test".to_string(), None, 120, 40, None)
            .await
            .unwrap();

        manager
            .write_stdin("history-test", "echo history-marker\r", 2000, 8000)
            .await
            .unwrap();
        let _ = manager
            .write_stdin("history-test", "", 200, 8000)
            .await
            .unwrap();

        let history = manager.read_history("history-test").await.unwrap();
        assert!(
            history.contains("history-marker"),
            "history missing marker: {}",
            history
        );

        manager.remove("history-test").await.unwrap();
    }

    #[tokio::test]
    async fn resize_updates_session_dimensions() {
        let manager = TerminalManager::new();
        manager
            .create("resize-test".to_string(), None, 80, 24, None)
            .await
            .unwrap();

        manager.resize("resize-test", 120, 50).await.unwrap();
        let info = manager
            .get("resize-test")
            .await
            .expect("missing terminal info");
        assert_eq!(info.cols, 120);
        assert_eq!(info.rows, 50);

        manager.remove("resize-test").await.unwrap();
    }

    #[tokio::test]
    async fn named_terminals_reject_duplicates_while_ephemeral_can_replace() {
        let manager = TerminalManager::new();
        manager
            .create_named("named-a".to_string(), None, 80, 24, None)
            .await
            .unwrap();

        let duplicate = manager
            .create_named("named-a".to_string(), None, 80, 24, None)
            .await;
        assert!(duplicate.is_err());
        assert!(duplicate
            .unwrap_err()
            .to_string()
            .contains("already exists"));

        manager
            .create("shell-a".to_string(), None, 80, 24, None)
            .await
            .unwrap();
        manager
            .create("shell-a".to_string(), None, 100, 30, None)
            .await
            .unwrap();
        let info = manager.get("shell-a").await.expect("missing shell-a");
        assert_eq!(info.cols, 100);
        assert_eq!(info.rows, 30);

        manager.remove("named-a").await.unwrap();
        manager.remove("shell-a").await.unwrap();
    }

    #[tokio::test]
    async fn cleanup_ephemeral_does_not_remove_named_sessions() {
        let manager = TerminalManager::new();
        manager
            .create_named("named-safe".to_string(), None, 80, 24, None)
            .await
            .unwrap();

        let removed = manager
            .close_ephemeral_idle_older_than(Duration::from_millis(0))
            .await;
        assert!(removed.is_empty(), "unexpected removals: {:?}", removed);
        assert!(manager.contains("named-safe").await);

        manager.remove("named-safe").await.unwrap();
    }
}
