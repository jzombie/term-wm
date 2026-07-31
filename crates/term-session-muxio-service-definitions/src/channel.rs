use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(not(target_os = "windows"))]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Clone)]
pub struct ChannelName {
    pub namespace: String,
    pub name: String,
}

impl ChannelName {
    pub fn parse(input: &str) -> Result<Self, String> {
        let input = input.trim();
        let parts: Vec<&str> = input.split('/').collect();
        let (ns, name) = match parts.as_slice() {
            [name] => ("default", *name),
            [ns, name] => (*ns, *name),
            _ => return Err(format!(
                "invalid channel format '{input}': expected 'name' or 'namespace/name'"
            )),
        };
        let is_valid = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        if !is_valid(ns) {
            return Err(format!(
                "invalid namespace '{ns}': must be non-empty alphanumeric, hyphen, or underscore"
            ));
        }
        if !is_valid(name) {
            return Err(format!(
                "invalid name '{name}': must be non-empty alphanumeric, hyphen, or underscore"
            ));
        }
        Ok(Self {
            namespace: ns.to_string(),
            name: name.to_string(),
        })
    }
}

impl fmt::Display for ChannelName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.namespace, self.name)
    }
}

#[derive(Debug, Clone)]
pub struct ChannelResolver {
    base_dir: PathBuf,
}

impl ChannelResolver {
    pub fn new(base_dir: Option<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.unwrap_or_else(Self::default_channels_dir),
        }
    }

    pub fn default_channels_dir() -> PathBuf {
        #[cfg(target_os = "windows")]
        {
            dirs::data_local_dir()
                .unwrap_or_else(|| std::env::temp_dir().join("term-wm"))
                .join("channels")
        }
        #[cfg(not(target_os = "windows"))]
        {
            let uid = unsafe { libc::getuid() };
            let base = dirs::data_dir().unwrap_or_else(|| {
                PathBuf::from(format!("/tmp/term-wm-{}", uid))
            });
            base.join("term-wm").join("channels")
        }
    }

    pub fn resolve(&self, channel: &ChannelName) -> io::Result<PathBuf> {
        let ns_dir = self.base_dir.join(&channel.namespace);
        fs::create_dir_all(&ns_dir)?;
        #[cfg(unix)]
        {
            fs::set_permissions(&ns_dir, fs::Permissions::from_mode(0o700))?;
        }
        let socket_path = ns_dir.join(format!("{}.sock", channel.name));
        let path_len = socket_path.to_string_lossy().as_bytes().len();
        if path_len >= 100 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "resolved path '{}' ({} bytes) exceeds POSIX 100-byte budget",
                    socket_path.display(),
                    path_len
                ),
            ));
        }
        Ok(socket_path)
    }
}

// ── IPC endpoint probing ──────────────────────────────────────────────

#[cfg(unix)]
pub fn probe_ipc_endpoint(path: &Path) -> bool {
    use std::os::unix::net::UnixStream;
    UnixStream::connect(path).is_ok()
}

#[cfg(windows)]
pub fn probe_ipc_endpoint(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Networking::WinSock::{
        closesocket, connect, socket, WSAStartup, WSACleanup, AF_UNIX, INVALID_SOCKET,
        SOCKADDR_UN, SOCKET, WSADATA, SOCK_STREAM,
    };

    let path16: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    if path16.len() > 108 {
        return false;
    }

    unsafe {
        let mut wsa_data: WSADATA = std::mem::zeroed();
        if WSAStartup(0x0202, &mut wsa_data) != 0 {
            return false;
        }
        let s: SOCKET = socket(AF_UNIX as i32, SOCK_STREAM as i32, 0);
        if s == INVALID_SOCKET {
            WSACleanup();
            return false;
        }
        let mut addr: SOCKADDR_UN = std::mem::zeroed();
        addr.sun_family = AF_UNIX as u16;
        for (i, &b) in path.to_string_lossy().as_bytes().iter().take(107).enumerate() {
            addr.sun_path[i] = b as i8;
        }
        let res = connect(
            s,
            &addr as *const _ as *const _,
            std::mem::size_of::<SOCKADDR_UN>() as i32,
        );
        closesocket(s);
        WSACleanup();
        res == 0
    }
}

// ── Sidecar lock ──────────────────────────────────────────────────────

#[cfg(unix)]
pub fn acquire_sidecar_lock(lock_path: &Path) -> io::Result<fs::File> {
    use std::os::unix::io::AsRawFd;

    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)?;
    let fd = file.as_raw_fd();
    let res = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if res != 0 {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "Another server is already running on this channel",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
pub fn acquire_sidecar_lock(lock_path: &Path) -> io::Result<fs::File> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATE,
    };

    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)?;
    let handle = file.as_raw_handle() as _;
    let mut overlapped = unsafe { std::mem::zeroed() };
    let flags = LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATE;
    let res = unsafe { LockFileEx(handle, flags, 0, 1, 0, &mut overlapped) };
    if res == 0 {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "Another server is already running on this channel",
        ));
    }
    Ok(file)
}
