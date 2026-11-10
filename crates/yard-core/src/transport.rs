//! The daemon's local socket. A Unix socket on Linux and macOS; on Windows a
//! localhost TCP port guarded by a random token, both written to a file only
//! the user can read.

use anyhow::{anyhow, Context, Result};
use std::io::{BufRead, BufReader, Write};

#[cfg(unix)]
pub type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
pub type Stream = std::net::TcpStream;

/// Where clients find the daemon.
pub fn endpoint() -> std::path::PathBuf {
    if cfg!(windows) {
        crate::paths::runtime_dir().join("yard.port")
    } else {
        crate::paths::socket_path()
    }
}

#[cfg(unix)]
pub fn connect() -> Result<Stream> {
    let path = endpoint();
    Stream::connect(&path).with_context(|| not_running(&path))
}

#[cfg(windows)]
pub fn connect() -> Result<Stream> {
    let path = endpoint();
    let text = std::fs::read_to_string(&path).with_context(|| not_running(&path))?;
    let mut it = text.split_whitespace();
    let port: u16 = it.next().and_then(|p| p.parse().ok()).ok_or_else(|| anyhow!("bad {}", path.display()))?;
    let token = it.next().unwrap_or_default().to_string();
    let mut s = Stream::connect(("127.0.0.1", port)).with_context(|| not_running(&path))?;
    s.write_all(format!("AUTH {token}\n").as_bytes())?;
    Ok(s)
}

fn not_running(path: &std::path::Path) -> String {
    format!(
        "cannot reach the Switchyard daemon at {} (start it with `yard setup`, or `yard daemon` in a terminal)",
        path.display()
    )
}

/// True when a daemon answers at the endpoint.
pub fn daemon_alive() -> bool {
    connect().is_ok()
}

pub struct Listener {
    #[cfg(unix)]
    inner: std::os::unix::net::UnixListener,
    #[cfg(windows)]
    inner: std::net::TcpListener,
    #[cfg(windows)]
    token: String,
}

impl Listener {
    pub fn bind() -> Result<Listener> {
        let dir = crate::paths::runtime_dir();
        std::fs::create_dir_all(&dir)?;
        let path = endpoint();
        if daemon_alive() {
            return Err(anyhow!("a Switchyard daemon is already running ({})", path.display()));
        }
        let _ = std::fs::remove_file(&path);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            let inner = std::os::unix::net::UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Listener { inner })
        }
        #[cfg(windows)]
        {
            let inner = std::net::TcpListener::bind(("127.0.0.1", 0))?;
            let port = inner.local_addr()?.port();
            let token = random_token();
            std::fs::write(&path, format!("{port} {token}\n"))?;
            Ok(Listener { inner, token })
        }
    }

    /// Blocks for the next client.
    pub fn accept(&self) -> std::io::Result<Stream> {
        self.inner.accept().map(|(s, _)| s)
    }

    /// Checks a new connection's credentials. On Unix the socket's file mode
    /// already did; on Windows the first line must carry the token.
    pub fn authorize(&self, reader: &mut BufReader<Stream>) -> bool {
        #[cfg(unix)]
        {
            let _ = reader;
            true
        }
        #[cfg(windows)]
        {
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                return false;
            }
            line.trim_end().strip_prefix("AUTH ").is_some_and(|t| t == self.token)
        }
    }

    pub fn cleanup(&self) {
        let _ = std::fs::remove_file(endpoint());
    }
}

#[cfg(windows)]
fn random_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..4u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i ^ std::process::id() as u64);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// Reads lines from a stream; on Windows the client already sent its token.
pub fn lines(s: Stream) -> std::io::Lines<BufReader<Stream>> {
    BufReader::new(s).lines()
}

#[allow(dead_code)]
fn _assert_traits() {
    fn is_rw<T: std::io::Read + Write>() {}
    is_rw::<Stream>();
}
