use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::sync::{mpsc, Notify};
use vt100::Parser;

use crate::sandbox::SandboxSession;

pub struct TerminalSession {
    pub name: String,
    parser: Parser,
    writer: Box<dyn Write + Send>,
    output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    output_notify: Arc<Notify>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _pty_pair: portable_pty::PtyPair,
    _reader_thread: std::thread::JoinHandle<()>,
    pub created_at: Instant,
    pub last_output_at: Instant,
    alive: Arc<AtomicBool>,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
}

impl TerminalSession {
    pub fn spawn(
        name: String,
        cwd: Option<PathBuf>,
        cols: u16,
        rows: u16,
        sandbox: Option<&SandboxSession>,
    ) -> Result<Self> {
        let pty_system = NativePtySystem::default();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY pair")?;

        let mut cmd = CommandBuilder::new("bash");
        cmd.env("TERM", "xterm-256color");
        cmd.env("PAGER", "cat");
        cmd.env("GIT_PAGER", "cat");
        cmd.env("GH_PAGER", "cat");
        cmd.env("LANG", "C.UTF-8");
        cmd.env("LC_ALL", "C.UTF-8");
        cmd.env("COLORTERM", "");

        let resolved_cwd: PathBuf;

        if let Some(sb) = sandbox {
            // Build: podman exec -it --workdir <workdir> <container> bash
            cmd = CommandBuilder::new("podman");
            cmd.arg("exec");
            cmd.arg("-it");
            cmd.arg("--workdir");
            let wd = match &cwd {
                Some(p) => p.display().to_string(),
                None => sb.workdir_display(),
            };
            cmd.arg(&wd);
            cmd.arg(sb.container_name());
            cmd.arg("bash");
            cmd.env("TERM", "xterm-256color");
            cmd.env("PAGER", "cat");
            cmd.env("GIT_PAGER", "cat");
            cmd.env("GH_PAGER", "cat");
            cmd.env("LANG", "C.UTF-8");
            cmd.env("LC_ALL", "C.UTF-8");
            cmd.env("COLORTERM", "");
            resolved_cwd = PathBuf::from(wd);
        } else {
            if let Some(ref p) = cwd {
                cmd.cwd(p);
            }
            resolved_cwd = match cwd {
                Some(p) => p,
                None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            };
        }

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn bash in PTY")?;

        let reader = pty_pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;
        let writer = pty_pair
            .master
            .take_writer()
            .context("Failed to take PTY writer")?;

        let parser = Parser::new(rows, cols, 0);
        let alive = Arc::new(AtomicBool::new(true));
        let alive_clone = alive.clone();
        let (tx, rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let notify = Arc::new(Notify::new());
        let notify_clone = notify.clone();

        let reader_thread = std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        alive_clone.store(false, Ordering::SeqCst);
                        break;
                    }
                    Ok(n) => {
                        let _ = tx.send(buf[..n].to_vec());
                        notify_clone.notify_one();
                    }
                }
            }
        });

        Ok(TerminalSession {
            name,
            parser,
            writer,
            output_rx: rx,
            output_notify: notify,
            child,
            _pty_pair: pty_pair,
            _reader_thread: reader_thread,
            created_at: Instant::now(),
            last_output_at: Instant::now(),
            alive,
            cwd: resolved_cwd,
            cols,
            rows,
        })
    }

    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer
            .write_all(data)
            .context("Failed to write to PTY")?;
        self.writer.flush().context("Failed to flush PTY")?;
        self.last_output_at = Instant::now();
        Ok(())
    }

    pub fn read_screen(&mut self) -> String {
        let mut had_output = false;
        while let Ok(chunk) = self.output_rx.try_recv() {
            self.parser.process(&chunk);
            had_output = true;
        }
        if had_output {
            self.last_output_at = Instant::now();
        }
        Self::screen_to_text(&self.parser)
    }

    /// Returns a reference to the Notify that fires when the reader thread
    /// pushes new output chunks into the channel.
    pub fn output_notify(&self) -> &Arc<Notify> {
        &self.output_notify
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn idle_duration(&self) -> Duration {
        self.last_output_at.elapsed()
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Check if the terminal application (e.g., less) has enabled
    /// application cursor keys mode (DECCKM). When true, arrow keys
    /// should use SS3 sequences (\x1bOA) instead of ANSI (\x1b[A).
    pub fn application_cursor_active(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    pub fn kill(&mut self) -> Result<()> {
        self.kill_process_group();
        self.reap_child();
        self.alive.store(false, Ordering::SeqCst);
        Ok(())
    }

    #[cfg(unix)]
    fn kill_process_group(&self) {
        if let Some(pid) = self.child.process_id() {
            unsafe {
                let pgid = libc::getpgid(pid as libc::pid_t);
                if pgid > 0 {
                    libc::kill(-pgid, libc::SIGTERM);
                    std::thread::sleep(Duration::from_millis(500));
                    if libc::kill(-pgid, 0) == 0 {
                        libc::kill(-pgid, libc::SIGKILL);
                    }
                }
            }
        }
    }

    #[cfg(not(unix))]
    fn kill_process_group(&self) {}

    fn reap_child(&mut self) {
        for _ in 0..10 {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        for _ in 0..20 {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }

    fn screen_to_text(parser: &Parser) -> String {
        let screen = parser.screen();
        let (_rows, cols) = screen.size();
        let rows: Vec<String> = screen.rows(0, cols).collect();
        let mut lines: Vec<String> = rows
            .into_iter()
            .map(|row| row.trim_end().to_string())
            .collect();
        while lines.last().map_or(false, |l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.writer.flush();
        self.alive.store(false, Ordering::SeqCst);
    }
}
