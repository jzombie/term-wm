use std::fs;
use std::io;
use std::path::Path;

use clap::Parser;
use term_session_muxio_service_definitions::{ChannelName, ChannelResolver};
use term_session_server::SessionServerConfig;

#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[derive(Parser, Debug)]
#[command(name = "term-session-server", about = "Pure PTY session manager")]
struct Cli {
    /// Channel name (namespace/name). Falls back to TERM_WM_CHANNEL env, then "default/main".
    #[arg(short, long)]
    channel: Option<String>,

    /// Columns (width) of each terminal
    #[arg(long = "cols", default_value = "80")]
    cols: u16,

    /// Rows (height) of each terminal
    #[arg(long = "rows", default_value = "24")]
    rows: u16,

    /// Command to run (and its arguments).
    /// If omitted, launches the default shell.
    #[arg(num_args = 0..)]
    cmd: Vec<String>,
}

#[cfg(unix)]
fn probe_ipc_endpoint(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}

#[cfg(windows)]
fn probe_ipc_endpoint(path: &Path) -> bool {
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

#[cfg(unix)]
fn acquire_sidecar_lock(lock_path: &Path) -> io::Result<fs::File> {
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
fn acquire_sidecar_lock(lock_path: &Path) -> io::Result<fs::File> {
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

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let channel_input = cli
        .channel
        .or_else(|| std::env::var("TERM_WM_CHANNEL").ok())
        .unwrap_or_else(|| "default/main".to_string());

    let channel = match ChannelName::parse(&channel_input) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Invalid channel: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let resolver = ChannelResolver::new(ChannelResolver::default_channels_dir());
    let socket_path = match resolver.resolve(&channel) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Failed to resolve channel socket: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    // Sidecar lock — only one server instance per channel
    let lock_path = socket_path.with_extension("sock.lock");
    if let Err(e) = acquire_sidecar_lock(&lock_path) {
        tracing::error!("{e}");
        return std::process::ExitCode::FAILURE;
    }

    // Stale socket cleanup (safe: we hold the exclusive lock)
    if socket_path.exists() && !probe_ipc_endpoint(&socket_path) {
        let _ = fs::remove_file(&socket_path);
    }

    let config = SessionServerConfig {
        channel: channel.clone(),
        socket_path: socket_path.to_string_lossy().to_string(),
        cmd: cli.cmd,
        cols: cli.cols,
        rows: cli.rows,
    };

    match term_session_server::run_server(config).await {
        Ok(code) => std::process::ExitCode::from(code as u8),
        Err(e) => {
            tracing::error!("SessionServer error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
