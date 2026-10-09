//! Transport of Agentty's local socket (`$AGENTTY_SOCKET`), which agent hooks, `agentty notify`,
//! `agentty browser` and the statusline wrapper talk to.
//!
//! - Unix (macOS, Linux): a Unix domain socket in the per-user temp dir, `0600`.
//! - Windows: the standard library has no Unix sockets there, so a loopback TCP port is used.
//!   Any local process could connect to it, so every connection must first send a token that only
//!   Agentty's own panes receive (`$AGENTTY_SOCKET_TOKEN`, one per pane; see `agent_signal`).
//!
//! Who is on the other end is decided in `agent_signal`; this module provides the facts: the
//! peer's process and user on Unix, the token line on Windows.

use std::io::{self, Read, Write};
use std::time::Duration;

/// Environment variable carrying the connection token (Windows only).
#[cfg_attr(unix, allow(dead_code))]
pub const TOKEN_VARIABLE: &str = "AGENTTY_SOCKET_TOKEN";

/// One connection.
pub struct Stream {
    #[cfg(unix)]
    inner: std::os::unix::net::UnixStream,
    #[cfg(not(unix))]
    inner: std::net::TcpStream,
}

impl Stream {
    pub fn try_clone(&self) -> io::Result<Stream> {
        Ok(Stream { inner: self.inner.try_clone()? })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_write_timeout(timeout)
    }

    /// Done writing (the other end sees end of input), reading stays open.
    pub fn shutdown_write(&self) {
        let _ = self.inner.shutdown(std::net::Shutdown::Write);
    }

    /// The process on the other end, as the kernel recorded it when it connected.
    #[cfg(target_os = "macos")]
    pub fn peer_pid(&self) -> Option<u32> {
        use std::os::fd::AsRawFd;
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        // SAFETY: `pid` and `len` are valid for writes of the sizes passed; the descriptor is open.
        let result = unsafe {
            libc::getsockopt(self.inner.as_raw_fd(), libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&mut pid as *mut libc::pid_t).cast(), &mut len)
        };
        (result == 0 && pid > 0).then_some(pid as u32)
    }

    #[cfg(target_os = "linux")]
    pub fn peer_pid(&self) -> Option<u32> {
        self.peer_credentials().map(|c| c.pid as u32).filter(|pid| *pid > 0)
    }

    #[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
    pub fn peer_pid(&self) -> Option<u32> {
        None
    }

    #[cfg(target_os = "linux")]
    fn peer_credentials(&self) -> Option<libc::ucred> {
        use std::os::fd::AsRawFd;
        let mut credentials = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: `credentials` and `len` are valid for writes of the sizes passed; the descriptor is open.
        let result = unsafe {
            libc::getsockopt(
                self.inner.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credentials as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        (result == 0).then_some(credentials)
    }

    /// Whether the peer runs as the same user (the socket's `0600` already says so; this holds even
    /// if the file mode were changed).
    #[cfg(unix)]
    pub fn peer_is_this_user(&self) -> bool {
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        #[cfg(target_os = "linux")]
        {
            self.peer_credentials().is_some_and(|c| c.uid == me)
        }
        #[cfg(not(target_os = "linux"))]
        {
            use std::os::fd::AsRawFd;
            let (mut uid, mut gid): (libc::uid_t, libc::gid_t) = (0, 0);
            // SAFETY: both pointers are valid for writes; the descriptor is open.
            let result = unsafe { libc::getpeereid(self.inner.as_raw_fd(), &mut uid, &mut gid) };
            result == 0 && uid == me
        }
    }

    /// Windows: the `auth\t<token>` line a connection opens with. Read byte by byte so nothing
    /// after it is consumed, with a short timeout and a length limit.
    #[cfg(not(unix))]
    pub fn read_token(&mut self) -> Option<String> {
        const LIMIT: usize = 128;
        self.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
        let mut line = Vec::with_capacity(LIMIT);
        let mut byte = [0u8; 1];
        loop {
            match self.read(&mut byte) {
                Ok(1) if byte[0] == b'\n' => break,
                Ok(1) if line.len() < LIMIT => line.push(byte[0]),
                _ => return None,
            }
        }
        String::from_utf8(line).ok()?.strip_prefix("auth\t").map(str::to_string)
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Connects to the socket at `address` (the value of `$AGENTTY_SOCKET`), with the token from
/// `$AGENTTY_SOCKET_TOKEN` on Windows.
pub fn connect(address: &str) -> io::Result<Stream> {
    connect_to(address, std::env::var(TOKEN_VARIABLE).ok().as_deref())
}

#[cfg(unix)]
pub fn connect_to(address: &str, _token: Option<&str>) -> io::Result<Stream> {
    Ok(Stream { inner: std::os::unix::net::UnixStream::connect(address)? })
}

#[cfg(not(unix))]
pub fn connect_to(address: &str, token: Option<&str>) -> io::Result<Stream> {
    let token = token.ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "$AGENTTY_SOCKET_TOKEN is not set"))?;
    let mut inner = std::net::TcpStream::connect_timeout(
        &address.parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad socket address"))?,
        Duration::from_secs(2),
    )?;
    let _ = inner.set_nodelay(true);
    writeln!(inner, "auth\t{token}")?;
    Ok(Stream { inner })
}

/// The listening side, owned by the app.
pub struct Listener {
    #[cfg(unix)]
    inner: std::os::unix::net::UnixListener,
    #[cfg(not(unix))]
    inner: std::net::TcpListener,
    /// What `$AGENTTY_SOCKET` is set to in panes.
    pub address: String,
    /// Windows: the token of a second Agentty launch (see `instance.rs`); panes get their own.
    pub token: Option<String>,
}

impl Listener {
    #[cfg(unix)]
    pub fn bind() -> io::Result<Listener> {
        let base = std::env::temp_dir();
        sweep_stale_socket_dirs(&base);
        Self::bind_path(private_socket_dir(&base)?.join(SOCKET_NAME))
    }

    /// Binds at `path`, whose folder must already keep other users out: `bind` creates the socket
    /// file with the process umask, and only the folder makes that harmless.
    #[cfg(unix)]
    pub(crate) fn bind_path(path: std::path::PathBuf) -> io::Result<Listener> {
        let _ = std::fs::remove_file(&path);
        let inner = std::os::unix::net::UnixListener::bind(&path)?;
        // Defense in depth: the folder already keeps other users out.
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(Listener { inner, address: path.display().to_string(), token: None })
    }

    #[cfg(not(unix))]
    pub fn bind() -> io::Result<Listener> {
        let inner = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let address = inner.local_addr()?.to_string();
        let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        Ok(Listener { inner, address, token: Some(token) })
    }

    /// Accepted connections, forever. Nothing is read here; `agent_signal` decides who each one is.
    pub fn incoming(&self) -> impl Iterator<Item = Stream> + '_ {
        self.inner.incoming().flatten().map(|inner| Stream { inner })
    }

    /// Removes the socket file and the private folder `bind` made for it (Unix).
    pub fn cleanup(address: &str) {
        #[cfg(unix)]
        {
            let path = std::path::Path::new(address);
            let _ = std::fs::remove_file(path);
            let ours = |p: &std::path::Path, prefix: &str| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(prefix));
            if let Some(dir) = path.parent().filter(|dir| ours(path, SOCKET_NAME) && ours(dir, "agentty-")) {
                // Only an empty folder goes: never anything else someone put there.
                let _ = std::fs::remove_dir(dir);
            }
        }
        #[cfg(not(unix))]
        let _ = address;
    }
}

/// The socket's file name inside its private folder.
#[cfg(unix)]
const SOCKET_NAME: &str = "agentty.sock";

/// Removes socket folders left in `base` by an Agentty that ended without cleaning up (killed or
/// crashed): only this user's own real folders, named as [`private_socket_dir`] names them, whose
/// process is gone, and only the socket inside (a folder with anything else in it stays).
#[cfg(unix)]
fn sweep_stale_socket_dirs(base: &std::path::Path) {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir(base) else { return };
    let me = unsafe { libc::getuid() };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(socket_dir_pid) else { continue };
        if pid == std::process::id() {
            continue;
        }
        let dir = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&dir) else { continue };
        if !meta.is_dir() || meta.uid() != me {
            continue;
        }
        // Signal 0 only checks: 0 or EPERM means the process still runs.
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if !alive {
            let _ = std::fs::remove_file(dir.join(SOCKET_NAME));
            let _ = std::fs::remove_dir(&dir);
        }
    }
}

/// The pid in a socket folder name `agentty-<pid>-<16 hex digits>`.
#[cfg(unix)]
fn socket_dir_pid(name: &str) -> Option<u32> {
    let (pid, random) = name.strip_prefix("agentty-")?.split_once('-')?;
    (random.len() == 16 && random.bytes().all(|b| b.is_ascii_hexdigit())).then_some(())?;
    pid.parse().ok()
}

/// A new folder `agentty-<pid>-<random>` in `base`, `0700` from the moment it exists. `bind`
/// creates the socket file with the process umask (and the umask is process-wide, so it can't be
/// narrowed for one call while other threads create files); inside this folder no other user can
/// reach the socket, not even for that moment. The folder is created, never reused: on Linux the
/// temp dir is the shared `/tmp`, where a folder another user made in advance must not be taken
/// for ours. Short enough for SUN_LEN under macOS's per-user temp dir.
#[cfg(unix)]
fn private_socket_dir(base: &std::path::Path) -> io::Result<std::path::PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let mut attempts = 0;
    loop {
        let random = uuid::Uuid::new_v4().simple().to_string();
        let dir = base.join(format!("agentty-{}-{}", std::process::id(), &random[..16]));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(dir),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists && attempts < 3 => attempts += 1,
            Err(err) => return Err(err),
        }
    }
}

/// Compares without stopping at the first difference, so response time doesn't reveal how much
/// of a guessed token was right.
#[cfg_attr(unix, allow(dead_code))]
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    fn compares_tokens() {
        assert!(constant_time_eq(b"auth\tabc\n", b"auth\tabc\n"));
        assert!(!constant_time_eq(b"auth\tabd\n", b"auth\tabc\n"));
        assert!(!constant_time_eq(b"auth\tab", b"auth\tabc\n"));
    }

    #[cfg(unix)]
    #[test]
    fn the_socket_lives_in_a_new_private_folder_removed_with_it() {
        use std::os::unix::fs::PermissionsExt;
        let listener = Listener::bind().unwrap();
        let path = std::path::PathBuf::from(&listener.address);
        let dir = path.parent().unwrap().to_path_buf();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        // Each listener gets a folder of its own.
        let second = Listener::bind().unwrap();
        assert_ne!(std::path::Path::new(&second.address).parent().unwrap(), dir);
        Listener::cleanup(&listener.address);
        Listener::cleanup(&second.address);
        assert!(!path.exists());
        assert!(!dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn only_socket_folders_of_ended_processes_are_swept() {
        let base = std::env::temp_dir().join(format!("agentty-ipc-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let random = "0123456789abcdef";
        // No process has this pid (above the largest pid any system hands out).
        let ended = base.join(format!("agentty-999999999-{random}"));
        let running = base.join(format!("agentty-{}-{random}", std::process::id()));
        let crowded = base.join(format!("agentty-999999998-{random}"));
        let unrelated = base.join("agentty-999999999-notours");
        for dir in [&ended, &running, &crowded, &unrelated] {
            std::fs::create_dir(dir).unwrap();
            std::fs::write(dir.join(SOCKET_NAME), b"").unwrap();
        }
        std::fs::write(crowded.join("keep.txt"), b"x").unwrap();
        sweep_stale_socket_dirs(&base);
        assert!(!ended.exists());
        assert!(running.join(SOCKET_NAME).exists());
        assert!(crowded.join("keep.txt").exists());
        assert!(unrelated.join(SOCKET_NAME).exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn socket_folder_names_are_parsed_strictly() {
        assert_eq!(socket_dir_pid("agentty-42-0123456789abcdef"), Some(42));
        assert_eq!(socket_dir_pid("agentty-42-0123456789abcde"), None);
        assert_eq!(socket_dir_pid("agentty-42.sock"), None);
        assert_eq!(socket_dir_pid("agentty-x-0123456789abcdef"), None);
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_leaves_other_folders_alone() {
        let dir = std::env::temp_dir().join(format!("agentty-ipc-keep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Not our socket name: the folder stays even when empty.
        let other = dir.join("other.sock");
        std::fs::write(&other, b"").unwrap();
        Listener::cleanup(&other.display().to_string());
        assert!(dir.is_dir());
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn roundtrip() {
        let listener = Listener::bind().unwrap();
        if let Some(token) = &listener.token {
            std::env::set_var(TOKEN_VARIABLE, token);
        }
        let address = listener.address.clone();
        // The client stays connected until the server has looked at it: the kernel can't name a
        // peer that has already hung up (hence `agent_signal::linger`).
        let (checked, wait) = std::sync::mpsc::channel::<()>();
        let client = std::thread::spawn(move || {
            let mut stream = connect(&address).unwrap();
            writeln!(stream, "hello").unwrap();
            let _ = wait.recv_timeout(std::time::Duration::from_secs(5));
        });
        #[cfg_attr(unix, allow(unused_mut))]
        let mut stream = listener.incoming().next().unwrap();
        #[cfg(unix)]
        {
            // The kernel names the peer: this very process, run by this user.
            assert!(stream.peer_is_this_user());
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            assert_eq!(stream.peer_pid(), Some(std::process::id()));
        }
        #[cfg(not(unix))]
        assert_eq!(stream.read_token().as_deref(), listener.token.as_deref());
        let line = BufReader::new(stream).lines().next().unwrap().unwrap();
        assert_eq!(line, "hello");
        let _ = checked.send(());
        client.join().unwrap();
        Listener::cleanup(&listener.address);
    }
}
