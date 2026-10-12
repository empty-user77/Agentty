//! Where the server listens, and one connection to it.
//!
//! On macOS and Linux the server listens on a Unix socket in a folder only this user can open,
//! and `tailscale serve` (which runs as root) is its only caller: nothing else on the Mac, no
//! other account and no web page, can reach it, and a socket can't be taken over after a crash
//! the way a freed port can. Where Tailscale can't open the socket (its sandboxed App Store build)
//! and on Windows, it listens on a free port of `127.0.0.1` instead, where everything that
//! connects still has to pass every check in `server`.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

/// What `tailscale serve` is pointed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Tcp(u16),
    #[cfg(unix)]
    Unix(PathBuf),
}

impl Endpoint {
    /// The port, when it is one.
    pub fn tcp_port(&self) -> Option<u16> {
        match self {
            Endpoint::Tcp(port) => Some(*port),
            #[cfg(unix)]
            Endpoint::Unix(_) => None,
        }
    }

    /// The target as `tailscale serve` takes it.
    pub fn serve_target(&self) -> String {
        match self {
            Endpoint::Tcp(port) => format!("http://127.0.0.1:{port}"),
            #[cfg(unix)]
            Endpoint::Unix(path) => format!("unix:{}", path.display()),
        }
    }
}

pub enum Listener {
    Tcp(TcpListener),
    #[cfg(unix)]
    /// The listener, its path, and the socket file's (device, inode): another server started at
    /// the same path since must keep its file when this one goes.
    Unix(UnixListener, PathBuf, (u64, u64)),
}

impl Listener {
    /// A free port on `127.0.0.1`.
    pub fn tcp() -> std::io::Result<Listener> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        Ok(Listener::Tcp(listener))
    }

    /// A socket at `path`, in a folder made (or narrowed) to this user alone.
    #[cfg(unix)]
    pub fn unix(path: &Path) -> std::io::Result<Listener> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let folder = path.parent().ok_or_else(|| std::io::Error::other("socket path has no folder"))?;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(folder)?;
        let meta = std::fs::symlink_metadata(folder)?;
        // Ours and a real folder: a link or someone else's folder could put the socket elsewhere.
        if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } {
            return Err(std::io::Error::other("socket folder is not this user's"));
        }
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
        // A socket left by a run that crashed.
        match std::fs::symlink_metadata(path) {
            Ok(_) => std::fs::remove_file(path)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let made = std::fs::symlink_metadata(path)?;
        Ok(Listener::Unix(listener, path.to_path_buf(), (made.dev(), made.ino())))
    }

    pub fn endpoint(&self) -> std::io::Result<Endpoint> {
        match self {
            Listener::Tcp(listener) => Ok(Endpoint::Tcp(listener.local_addr()?.port())),
            #[cfg(unix)]
            Listener::Unix(_, path, _) => Ok(Endpoint::Unix(path.clone())),
        }
    }

    /// The next connection: `Ok(None)` when none is waiting. A TCP connection from anywhere but
    /// this machine is dropped (the listener is on loopback, so none should come).
    pub fn accept(&self) -> std::io::Result<Option<Conn>> {
        let accepted = match self {
            Listener::Tcp(listener) => listener.accept().map(|(stream, peer)| peer.ip().is_loopback().then_some(Conn::Tcp(stream))),
            #[cfg(unix)]
            Listener::Unix(listener, _, _) => listener.accept().map(|(stream, _)| Some(Conn::Unix(stream))),
        };
        match accepted {
            Ok(conn) => Ok(conn),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Listener::Unix(_, path, made) = self {
            use std::os::unix::fs::MetadataExt;
            // Only the file this server made: a newer one may have taken the path over.
            if std::fs::symlink_metadata(&path).is_ok_and(|now| (now.dev(), now.ino()) == *made) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

pub enum Conn {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

impl Conn {
    pub fn set_nonblocking(&self, on: bool) -> std::io::Result<()> {
        match self {
            Conn::Tcp(s) => s.set_nonblocking(on),
            #[cfg(unix)]
            Conn::Unix(s) => s.set_nonblocking(on),
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        match self {
            Conn::Tcp(s) => s.set_read_timeout(timeout),
            #[cfg(unix)]
            Conn::Unix(s) => s.set_read_timeout(timeout),
        }
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        match self {
            Conn::Tcp(s) => s.set_write_timeout(timeout),
            #[cfg(unix)]
            Conn::Unix(s) => s.set_write_timeout(timeout),
        }
    }

    /// Ends this side's writing (FIN) while the other side may still send.
    pub fn shutdown_write(&self) -> std::io::Result<()> {
        match self {
            Conn::Tcp(s) => s.shutdown(std::net::Shutdown::Write),
            #[cfg(unix)]
            Conn::Unix(s) => s.shutdown(std::net::Shutdown::Write),
        }
    }

    /// Whether the other end closed. A write to a closed connection still succeeds once (the
    /// reset comes back after it), so this looks for the end of the stream instead, without
    /// waiting and without touching the socket's blocking mode.
    pub fn peer_gone(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let fd = match self {
                Conn::Tcp(s) => s.as_raw_fd(),
                Conn::Unix(s) => s.as_raw_fd(),
            };
            let mut byte = 0u8;
            // SAFETY: a one-byte peek into a local buffer on a socket this value owns.
            let n = unsafe { libc::recv(fd, (&mut byte as *mut u8).cast(), 1, libc::MSG_PEEK | libc::MSG_DONTWAIT) };
            match n {
                0 => true,
                n if n > 0 => false,
                _ => {
                    let error = std::io::Error::last_os_error();
                    !matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted)
                }
            }
        }
        #[cfg(not(unix))]
        {
            let Conn::Tcp(stream) = self;
            if stream.set_nonblocking(true).is_err() {
                return true;
            }
            let gone = match stream.peek(&mut [0u8; 1]) {
                Ok(0) => true,
                Ok(_) => false,
                Err(e) => e.kind() != std::io::ErrorKind::WouldBlock,
            };
            stream.set_nonblocking(false).is_err() || gone
        }
    }

    /// Writes `bytes` by `deadline`, all of them or none counted: a reader taking one byte every
    /// few seconds must not hold the connection past it (each write's own timeout restarts).
    pub fn write_by(&mut self, bytes: &[u8], deadline: Instant) -> std::io::Result<()> {
        for chunk in bytes.chunks(16 * 1024) {
            let mut chunk = chunk;
            while !chunk.is_empty() {
                let left = deadline
                    .checked_duration_since(Instant::now())
                    .filter(|d| !d.is_zero())
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::TimedOut))?;
                self.set_write_timeout(Some(left))?;
                match self.write(chunk) {
                    Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
                    Ok(n) => chunk = &chunk[n..],
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
        }
        self.flush()
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Tcp(s) => s.read(buf),
            #[cfg(unix)]
            Conn::Unix(s) => s.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Tcp(s) => s.write(buf),
            #[cfg(unix)]
            Conn::Unix(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Tcp(s) => s.flush(),
            #[cfg(unix)]
            Conn::Unix(s) => s.flush(),
        }
    }
}

impl super::http::Deadline for Conn {
    fn limit_next_read(&mut self, left: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(left.max(Duration::from_millis(1))))
    }
}

/// Where the socket goes for this data folder: `<data>/remote/web.sock`, or, when that path is
/// too long for a socket address (about 100 bytes), a folder of its own in this user's private
/// temporary folder, named after the data folder so two instances never share one.
#[cfg(unix)]
pub fn socket_path(data_dir: &Path) -> PathBuf {
    let preferred = data_dir.join("remote").join("web.sock");
    if preferred.as_os_str().len() <= 100 {
        return preferred;
    }
    let tag: String = ring::digest::digest(&ring::digest::SHA256, data_dir.as_os_str().as_encoded_bytes()).as_ref()[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::env::temp_dir().join(format!("agentty-remote-{tag}")).join("web.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn socket_in_a_private_folder() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("agentty-conn-test-{}", std::process::id()));
        let path = base.join("remote").join("web.sock");
        let listener = Listener::unix(&path).unwrap();
        let folder = std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
        let socket = std::fs::symlink_metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!((folder, socket), (0o700, 0o600));
        assert_eq!(listener.endpoint().unwrap().serve_target(), format!("unix:{}", path.display()));
        // A second server at the same path (a restart) takes it over; the first one going
        // away later must not take the second one's socket with it.
        let second = Listener::unix(&path).unwrap();
        drop(listener);
        assert!(path.exists(), "the newer server keeps its socket");
        drop(second);
        assert!(!path.exists(), "removed when its own server stops");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn long_data_folders_get_a_short_socket() {
        let long = Path::new("/tmp").join("x".repeat(120));
        let path = socket_path(&long);
        assert!(path.as_os_str().len() < 104, "{}", path.display());
        assert_ne!(path, socket_path(&Path::new("/tmp").join("y".repeat(120))), "one per data folder");
        assert_eq!(socket_path(Path::new("/Users/me/.agentty")), Path::new("/Users/me/.agentty/remote/web.sock"));
    }

    #[test]
    fn notices_a_closed_peer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let server = Conn::Tcp(server);
        assert!(!server.peer_gone(), "still there");
        drop(client);
        std::thread::sleep(Duration::from_millis(50));
        assert!(server.peer_gone(), "closed its end");
    }

    #[test]
    fn writes_by_a_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let mut server = Conn::Tcp(server);
        // Nobody reads: the socket buffers fill and the deadline ends the write.
        let started = Instant::now();
        let result = server.write_by(&vec![0u8; 32 * 1024 * 1024], Instant::now() + Duration::from_millis(300));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
