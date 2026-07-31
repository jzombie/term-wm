use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use term_session_muxio_service_definitions::{
    ChannelName, ChannelResolver, probe_ipc_endpoint,
};

/// Parameters forwarded to the auto-spawned server process.
#[derive(Clone, Debug)]
pub struct ServerSpawnConfig<'a> {
    pub channel: &'a ChannelName,
    pub base_dir: Option<&'a Path>,
    pub cols: u16,
    pub rows: u16,
    pub cmd: &'a [String],
}

fn spawn_detached_server(cfg: &ServerSpawnConfig<'_>) -> io::Result<Child> {
    let bin = std::env::current_exe()?;
    let mut cmd = Command::new(bin);
    cmd.arg("--server").arg("--channel").arg(cfg.channel.to_string());
    if let Some(dir) = cfg.base_dir {
        cmd.arg("--base-dir").arg(dir);
    }
    cmd.arg("--cols").arg(cfg.cols.to_string());
    cmd.arg("--rows").arg(cfg.rows.to_string());
    if !cfg.cmd.is_empty() {
        cmd.arg("--").args(cfg.cmd);
    }
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
    cfg: &ServerSpawnConfig<'_>,
) -> io::Result<PathBuf> {
    let socket_path = resolver.resolve(channel)?;

    if probe_ipc_endpoint(&socket_path) {
        return Ok(socket_path);
    }

    let mut child = spawn_detached_server(cfg)?;
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
