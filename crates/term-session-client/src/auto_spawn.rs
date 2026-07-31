use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use term_session_muxio_service_definitions::{ChannelName, ChannelResolver};

#[cfg(unix)]
fn probe_ipc_endpoint(path: &Path) -> bool {
    use std::os::unix::net::UnixStream;
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

fn spawn_detached_server(channel: &ChannelName) -> io::Result<Child> {
    let binary_name = format!("term-session-server{}", std::env::consts::EXE_SUFFIX);
    let server_binary = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(&binary_name)))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(binary_name));

    let mut cmd = Command::new(server_binary);
    cmd.arg("--channel").arg(channel.to_string());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.spawn()
}

pub fn connect_or_spawn_server(
    channel: &ChannelName,
    resolver: &ChannelResolver,
) -> io::Result<PathBuf> {
    let socket_path = resolver.resolve(channel)?;

    if probe_ipc_endpoint(&socket_path) {
        return Ok(socket_path);
    }

    let mut child = spawn_detached_server(channel)?;
    let start = Instant::now();
    let timeout = Duration::from_secs(3);
    let poll_interval = Duration::from_millis(50);

    while start.elapsed() < timeout {
        if probe_ipc_endpoint(&socket_path) {
            return Ok(socket_path);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("Server exited prematurely during startup with status: {status}"),
            ));
        }
        thread::sleep(poll_interval);
    }

    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("Timed out waiting for server on channel '{channel}'"),
    ))
}
