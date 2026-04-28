use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::sandbox::SandboxSession;

use super::keyparse::parse_keys;
use super::session::TerminalSession;
use super::{TerminalInfo, TerminalOutput};

#[derive(Clone)]
pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, TerminalSession>>>,
    max_sessions: usize,
}

impl TerminalManager {
    pub fn new() -> Self {
        TerminalManager {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            max_sessions: 16,
        }
    }

    pub fn create(
        &self,
        name: String,
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
        sandbox: Option<&SandboxSession>,
    ) -> Result<TerminalInfo> {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(mut old) = sessions.remove(&name) {
            let _ = old.kill();
        }
        while sessions.len() >= self.max_sessions {
            let oldest_key = sessions
                .iter()
                .min_by_key(|(_, s)| s.created_at)
                .map(|(k, _)| k.clone());
            if let Some(key) = oldest_key {
                if let Some(mut s) = sessions.remove(&key) {
                    let _ = s.kill();
                }
            } else {
                break;
            }
        }
        let session = TerminalSession::spawn(name.clone(), cwd, cols, rows, sandbox)?;
        let info = self.session_info(&name, &session);
        sessions.insert(name, session);
        Ok(info)
    }

    pub fn write_stdin(
        &self,
        name: &str,
        input: &str,
        yield_ms: u64,
        max_output: usize,
    ) -> Result<TerminalOutput> {
        let start = Instant::now();
        let bytes = parse_keys(input);
        {
            let mut sessions = self.sessions.lock().unwrap();
            let session = sessions
                .get_mut(name)
                .with_context(|| format!("terminal session '{}' not found", name))?;
            let bytes = if session.application_cursor_active() {
                translate_cursor_keys_to_application(&bytes)
            } else {
                bytes
            };
            session.write(&bytes)?;
        }
        std::thread::sleep(Duration::from_millis(100));
        let final_output = self.poll_until_stable(name, yield_ms, start);
        let (output_text, truncated) = head_tail_truncate(&final_output, max_output);
        Ok(TerminalOutput {
            output: output_text,
            exit_code: None,
            session_name: Some(name.to_string()),
            wall_time_ms: start.elapsed().as_millis() as u64,
            output_truncated: truncated,
        })
    }

    pub fn exec_one_shot(
        &self,
        cmd: &str,
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
        yield_ms: u64,
        max_output: usize,
        sandbox: Option<&SandboxSession>,
    ) -> Result<TerminalOutput> {
        let start = Instant::now();
        let temp_name = format!(
            "_oneshot_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        let mut session = TerminalSession::spawn(temp_name, cwd, cols, rows, sandbox)?;
        let bytes = parse_keys(&format!("{}\n", cmd));
        session.write(&bytes)?;
        std::thread::sleep(Duration::from_millis(100));

        let mut last = String::new();
        let mut stable = 0u32;
        let deadline = start + Duration::from_millis(yield_ms);
        loop {
            std::thread::sleep(Duration::from_millis(100));
            let current = session.read_screen();
            if current == last {
                stable += 1;
                if stable >= 3 {
                    break;
                }
            } else {
                last = current;
                stable = 0;
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        let final_output = session.read_screen();
        let (output_text, truncated) = head_tail_truncate(&final_output, max_output);
        let _ = session.kill();
        Ok(TerminalOutput {
            output: output_text,
            exit_code: None,
            session_name: None,
            wall_time_ms: start.elapsed().as_millis() as u64,
            output_truncated: truncated,
        })
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(mut session) = sessions.remove(name) {
            session.kill()?;
        }
        Ok(())
    }

    pub fn remove_all(&self) {
        let mut sessions = self.sessions.lock().unwrap();
        for (_, mut session) in sessions.drain() {
            let _ = session.kill();
        }
    }

    pub fn list(&self) -> Vec<TerminalInfo> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .iter()
            .map(|(name, s)| self.session_info(name, s))
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<TerminalInfo> {
        let sessions = self.sessions.lock().unwrap();
        sessions.get(name).map(|s| self.session_info(&s.name, s))
    }

    fn session_info(&self, name: &str, session: &TerminalSession) -> TerminalInfo {
        TerminalInfo {
            name: name.to_string(),
            cwd: session.cwd.clone(),
            cols: session.cols,
            rows: session.rows,
            alive: session.is_alive(),
            idle_ms: session.idle_duration().as_millis() as u64,
            pid: session.pid(),
        }
    }

    fn poll_until_stable(&self, name: &str, yield_ms: u64, start: Instant) -> String {
        let mut last = String::new();
        let mut stable = 0u32;
        let deadline = start + Duration::from_millis(yield_ms);
        loop {
            std::thread::sleep(Duration::from_millis(100));
            let current = {
                let mut sessions = self.sessions.lock().unwrap();
                match sessions.get_mut(name) {
                    Some(session) => session.read_screen(),
                    None => return last,
                }
            };
            if current == last {
                stable += 1;
                if stable >= 3 {
                    return current;
                }
            } else {
                last = current;
                stable = 0;
            }
            if Instant::now() >= deadline {
                return last;
            }
        }
    }
}

/// Translate ANSI cursor key sequences to application (SS3) sequences.
/// ANSI: ESC [ A/B/C/D  →  SS3: ESC O A/B/C/D
fn translate_cursor_keys_to_application(data: &[u8]) -> Vec<u8> {
    if data.len() < 3 {
        return data.to_vec();
    }
    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if i + 2 < data.len()
            && data[i] == 0x1b
            && data[i + 1] == 0x5b // '['
            && (data[i + 2] == 0x41
                || data[i + 2] == 0x42
                || data[i + 2] == 0x43
                || data[i + 2] == 0x44)
        {
            // ANSI cursor: ESC [ X  →  SS3 cursor: ESC O X
            result.push(0x1b);
            result.push(0x4f); // 'O' instead of '['
            result.push(data[i + 2]);
            i += 3;
        } else {
            result.push(data[i]);
            i += 1;
        }
    }
    result
}

fn head_tail_truncate(text: &str, max_chars: usize) -> (String, bool) {
    if text.len() <= max_chars {
        return (text.to_string(), false);
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
        "{}…\n…[{} chars truncated]…\n{}",
        head,
        text.len().saturating_sub(max_chars),
        &text[tail_start..]
    );
    (truncated, true)
}
