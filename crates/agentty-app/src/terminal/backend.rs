//! PTY + VT state machine. `alacritty_terminal` owns parsing and the grid; its I/O thread reads the
//! PTY and reports through [`Listener`], which forwards events to the UI thread over a channel.

use crate::launch::LaunchSpec;
use alacritty_terminal::event::{Event, EventListener, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use anyhow::{Context as _, Result};
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone)]
pub struct Listener(UnboundedSender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.unbounded_send(event);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub columns: usize,
    pub lines: usize,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

impl GridSize {
    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.lines as u16,
            num_cols: self.columns as u16,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }
}

pub struct Backend {
    pub term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    size: GridSize,
    pub child_pid: u32,
    /// PTY controller descriptor, owned by the event loop and valid while this backend lives
    /// (`-1` on Windows, where ConPTY has no such descriptor).
    pub tty_fd: crate::procinfo::TtyFd,
}

pub struct SpawnOptions<'a> {
    pub spec: &'a LaunchSpec,
    pub pane_id: u64,
    /// `$AGENTTY_SOCKET` and, on Windows, `$AGENTTY_SOCKET_TOKEN`.
    pub signal_socket: Option<(&'a str, Option<&'a str>)>,
    pub scrollback: usize,
}

static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);

impl Backend {
    pub fn spawn(options: SpawnOptions, size: GridSize) -> Result<(Self, UnboundedReceiver<Event>)> {
        let spec = options.spec;
        let (tx, rx) = unbounded();
        let listener = Listener(tx);

        // Credentials chosen in Settings → Accounts (nothing for the agents' own CLI login).
        let auth = spec.auth_setup();
        let (program, args) = spec.argv_with_auth(&auth);
        let mut env = HashMap::new();
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "Agentty".into());
        env.insert("TERM_PROGRAM_VERSION".into(), env!("CARGO_PKG_VERSION").into());
        env.insert("AGENTTY_PANE_ID".into(), options.pane_id.to_string());
        // `agentty browser …` / `agentty notify …` work in every pane.
        if let Ok(exe) = std::env::current_exe() {
            env.insert("AGENTTY_BIN".into(), exe.display().to_string());
            // Windows: this process's PATH plus what the registry has now (tools installed since
            // Agentty started); elsewhere `$PATH` as before.
            let path = if cfg!(windows) { Some(agentty_bridge::process::current_path()) } else { std::env::var_os("PATH") };
            if let (Some(dir), Some(path)) = (exe.parent(), path) {
                // macOS: the app bundle's own folder goes first. Elsewhere the executable may sit
                // in Downloads or another shared folder, so it goes last: a `git.exe` or `node`
                // lying next to it must never shadow the real ones.
                let mut paths: Vec<std::path::PathBuf> = std::env::split_paths(&path).collect();
                if cfg!(target_os = "macos") {
                    paths.insert(0, dir.to_path_buf());
                } else {
                    paths.push(dir.to_path_buf());
                }
                if let Ok(joined) = std::env::join_paths(paths) {
                    env.insert("PATH".into(), joined.to_string_lossy().to_string());
                }
            }
        }
        // Claude Code on Windows runs hooks with Git Bash; point it at Git for Windows' bash so
        // WSL's `bash.exe` (or none) isn't picked, which would silence pane status.
        #[cfg(windows)]
        if std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").is_none() {
            if let Some(bash) = agentty_bridge::process::git_bash() {
                env.insert("CLAUDE_CODE_GIT_BASH_PATH".into(), bash.display().to_string());
            }
        }
        env.extend(crate::shell_integration::environment(&program));
        env.extend(auth.pane_variables());
        if let Some((address, token)) = options.signal_socket {
            env.insert("AGENTTY_SOCKET".into(), address.to_string());
            if let Some(token) = token {
                env.insert(crate::ipc::TOKEN_VARIABLE.into(), token.to_string());
            }
        }
        let scrollback = options.scrollback;
        let options = tty::Options {
            shell: Some(tty::Shell::new(program, args)),
            working_directory: Some(spec.cwd.clone()),
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: true,
        };

        let config = Config { scrolling_history: scrollback, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let window_id = NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed);
        let pty = tty::new(&options, size.window_size(), window_id).context("failed to open PTY")?;
        #[cfg(unix)]
        let (child_pid, tty_fd) = (pty.child().id(), std::os::fd::AsRawFd::as_raw_fd(pty.file()));
        #[cfg(not(unix))]
        let (child_pid, tty_fd) = (pty.child_watcher().pid().map_or(0, |pid| pid.get()), -1);
        let event_loop =
            EventLoop::new(term.clone(), listener, pty, options.drain_on_exit, false).context("failed to start PTY event loop")?;
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();

        Ok((Self { term, notifier, size, child_pid, tty_fd }, rx))
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if !bytes.is_empty() {
            let _ = self.notifier.0.send(Msg::Input(bytes));
        }
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    pub fn resize(&mut self, size: GridSize) {
        if size == self.size || size.columns == 0 || size.lines == 0 {
            return;
        }
        self.size = size;
        self.notifier.on_resize(size.window_size());
        self.term.lock().resize(size);
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        // Closing the pane ends what runs in it. Some agents (Claude Code, Codex) ignore the hangup
        // the closed terminal sends and would keep running without a terminal, so signal the
        // foreground job and the shell's group directly.
        #[cfg(unix)]
        unsafe {
            let foreground = libc::tcgetpgrp(self.tty_fd);
            // The shell's pid is only signalled while it still leads its own group: once it exited and
            // was reaped, the pid (and group id) may belong to an unrelated process.
            let shell = self.child_pid as libc::pid_t;
            let shell_group = if libc::getpgid(shell) == shell { shell } else { -1 };
            let own = libc::getpgrp();
            for group in [foreground, shell_group] {
                if group > 1 && group != own {
                    libc::killpg(group, libc::SIGHUP);
                    libc::killpg(group, libc::SIGTERM);
                }
            }
        }
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}
