//! Transport of Agentty's local socket (`$AGENTTY_SOCKET`), which agent hooks, `agentty notify`,
//! `agentty browser` and the statusline wrapper talk to.
//!
//! - Unix (macOS, Linux): a Unix domain socket in the per-user temp dir, `0600`.
//! - Windows: the standard library has no Unix sockets there, so a loopback TCP port is used.
//!   Any local process could connect to it, so every connection must first send the per-launch
//!   token that only Agentty's own panes receive (`$AGENTTY_SOCKET_TOKEN`).

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
    /// What `$AGENTTY_SOCKET_TOKEN` is set to in panes (Windows).
    pub token: Option<String>,
}

impl Listener {
    #[cfg(unix)]
    pub fn bind() -> io::Result<Listener> {
        // The per-user temp dir is private (0700) and short enough for SUN_LEN, unlike deep data dirs.
        Self::bind_path(std::env::temp_dir().join(format!("agentty-{}.sock", std::process::id())))
    }

    #[cfg(unix)]
    fn bind_path(path: std::path::PathBuf) -> io::Result<Listener> {
        let _ = std::fs::remove_file(&path);
        let inner = std::os::unix::net::UnixListener::bind(&path)?;
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

    /// Accepted connections, forever. On Windows, connections that don't open with the token are
    /// dropped here, before anything they send is read by the app.
    pub fn incoming(&self) -> impl Iterator<Item = Stream> + '_ {
        self.inner.incoming().flatten().filter_map(move |inner| {
            let stream = Stream { inner };
            self.authorize(stream)
        })
    }

    #[cfg(unix)]
    fn authorize(&self, stream: Stream) -> Option<Stream> {
        Some(stream)
    }

    #[cfg(not(unix))]
    fn authorize(&self, mut stream: Stream) -> Option<Stream> {
        let expected = format!("auth\t{}\n", self.token.as_deref()?);
        stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
        // Byte by byte so nothing after the token line is consumed here.
        let mut line = Vec::with_capacity(expected.len());
        let mut byte = [0u8; 1];
        while line.len() < expected.len() {
            match stream.read(&mut byte) {
                Ok(1) => line.push(byte[0]),
                _ => return None,
            }
            if byte[0] == b'\n' {
                break;
            }
        }
        constant_time_eq(&line, expected.as_bytes()).then_some(stream)
    }

    /// Removes the socket file (Unix).
    pub fn cleanup(address: &str) {
        #[cfg(unix)]
        let _ = std::fs::remove_file(address);
        #[cfg(not(unix))]
        let _ = address;
    }
}

/// Compares without stopping at the first difference, so response time doesn't reveal how much
/// of a guessed token was right.
#[cfg_attr(unix, allow(dead_code))]
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
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

    #[test]
    fn roundtrip() {
        #[cfg(unix)]
        let listener = Listener::bind_path(std::env::temp_dir().join(format!("agentty-ipc-test-{}.sock", std::process::id()))).unwrap();
        #[cfg(not(unix))]
        let listener = Listener::bind().unwrap();
        if let Some(token) = &listener.token {
            std::env::set_var(TOKEN_VARIABLE, token);
        }
        let address = listener.address.clone();
        let client = std::thread::spawn(move || {
            let mut stream = connect(&address).unwrap();
            writeln!(stream, "hello").unwrap();
        });
        let stream = listener.incoming().next().unwrap();
        let line = BufReader::new(stream).lines().next().unwrap().unwrap();
        assert_eq!(line, "hello");
        client.join().unwrap();
        Listener::cleanup(&listener.address);
    }
}
