use std::fmt;
use std::io;
use std::thread;
use std::time::{Duration, Instant};

use term_session_muxio_service_definitions::{
    ChannelName, gateway_channel_name, probe_ipc_endpoint,
};

#[cfg(unix)]
use std::process::{Child, Command, Stdio};

/// Resolve the gateway channel name to probe/spawn.
/// Uses the runtime `TERM_WM_GATEWAY` override if present, else the static
/// user-scoped default (`term-wm/<user>/gateway`).
pub fn resolve_gateway() -> ChannelName {
    gateway_channel_name()
}

/// Handle to a just-spawned daemon process, used to poll for early death during
/// the gateway startup handshake.
///
/// Windows has no `setsid()`; the daemon there is spawned with a raw
/// `CreateProcessW(..., bInheritHandles = FALSE, ...)` so it can never inherit
/// the parent's console, pipes, or sockets — the analogue of the Unix
/// detachment. Because `std::process::Child` cannot be built from a raw process
/// handle on stable Rust, this wrapper holds either the std child (unix) or the
/// raw process handles (windows) behind a common `try_wait`.
enum DaemonChild {
    #[cfg(unix)]
    Unix(Child),
    #[cfg(windows)]
    Windows(WindowsDaemonProcess),
}

/// How a daemon process ended, rendered into the startup-failure message.
enum DaemonExitStatus {
    #[cfg(unix)]
    Unix(std::process::ExitStatus),
    #[cfg(windows)]
    Windows(u32),
}

impl fmt::Display for DaemonExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(unix)]
            DaemonExitStatus::Unix(status) => write!(f, "{status}"),
            #[cfg(windows)]
            DaemonExitStatus::Windows(code) => write!(f, "exit code: {code}"),
        }
    }
}

impl DaemonChild {
    /// Poll for daemon exit without blocking. Returns `Some` if the process has
    /// already exited, `None` if it is still running.
    fn try_wait(&mut self) -> io::Result<Option<DaemonExitStatus>> {
        match self {
            #[cfg(unix)]
            DaemonChild::Unix(child) => {
                Ok(child.try_wait()?.map(DaemonExitStatus::Unix))
            }
            #[cfg(windows)]
            DaemonChild::Windows(proc) => proc.try_wait(),
        }
    }
}

/// Owned Windows process handles for the detached daemon. `process` is polled
/// for early exit; both handles are closed on drop.
#[cfg(windows)]
struct WindowsDaemonProcess {
    process: windows_sys::Win32::Foundation::HANDLE,
    thread: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl WindowsDaemonProcess {
    fn try_wait(&mut self) -> io::Result<Option<DaemonExitStatus>> {
        use windows_sys::Win32::Foundation::{
            WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
        };
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, WaitForSingleObject,
        };
        unsafe {
            match WaitForSingleObject(self.process, 0) {
                WAIT_TIMEOUT => Ok(None),
                WAIT_OBJECT_0 => {
                    let mut code = 0u32;
                    if GetExitCodeProcess(self.process, &mut code) == 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(Some(DaemonExitStatus::Windows(code)))
                }
                WAIT_FAILED => Err(io::Error::last_os_error()),
                _ => Err(io::Error::last_os_error()),
            }
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsDaemonProcess {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        unsafe {
            let _ = CloseHandle(self.process);
            let _ = CloseHandle(self.thread);
        }
    }
}

fn spawn_detached_server(bin: &std::path::Path) -> io::Result<DaemonChild> {
    #[cfg(unix)]
    {
        unix_spawn_detached_server(bin)
    }
    #[cfg(windows)]
    {
        windows_spawn_detached_server(bin)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "daemon detachment is not supported on this platform",
        ))
    }
}

#[cfg(unix)]
fn unix_spawn_detached_server(bin: &std::path::Path) -> io::Result<DaemonChild> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(bin);
    cmd.arg("--daemon");
    // All stdio is detached: a daemon must not rely on the parent reading its
    // pipes. In particular, a piped stderr that is never drained lets the OS
    // pipe buffer fill, blocking the server's stderr writes and deadlocking
    // startup on every platform. Discard it instead.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Start the server in its own session and process group via setsid().
    // This is the only process-group manipulation done here: a child that
    // already became a process-group leader (e.g. via setpgid) would have
    // setsid() fail with EPERM. Detaching from the launching terminal means
    // the daemon can never freeze its input, and terminal Ctrl+C / Ctrl+Z /
    // SIGHUP-on-close are never delivered to it.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn().map(DaemonChild::Unix)
}

#[cfg(windows)]
fn windows_spawn_detached_server(bin: &std::path::Path) -> io::Result<DaemonChild> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
        PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW,
    };

    // The Unix side detaches via setsid(); on Windows the same guarantee needs
    // bInheritHandles = FALSE, which std::process::Command never passes (it
    // always spawns with TRUE so its own stdio handles are inherited, making
    // CREATE_NO_INHERIT a no-op). Spawn with raw CreateProcessW instead:
    // DETACHED_PROCESS removes the console (no CTRL_CLOSE_EVENT ever reaches
    // the daemon) and CREATE_NEW_PROCESS_GROUP isolates it from Ctrl+C,
    // mirroring the Unix session/process-group split.
    let nul_path: Vec<u16> = "\\\\.\\NUL\0".encode_utf16().collect();
    let nul_handle = unsafe {
        CreateFileW(
            nul_path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if nul_handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }

    // Point stdio at NUL so the daemon's standard streams never block on an
    // undrained pipe (the stderr-deadlock hazard noted on the unix side). With
    // bInheritHandles = FALSE these handles are NOT inherited as stray handles;
    // STARTF_USESTDHANDLES only selects them for the standard-handle slots.
    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpReserved: std::ptr::null_mut(),
        lpDesktop: std::ptr::null_mut(),
        lpTitle: std::ptr::null_mut(),
        dwX: 0,
        dwY: 0,
        dwXSize: 0,
        dwYSize: 0,
        dwXCountChars: 0,
        dwYCountChars: 0,
        dwFillAttribute: 0,
        dwFlags: STARTF_USESTDHANDLES,
        wShowWindow: 0,
        cbReserved2: 0,
        lpReserved2: std::ptr::null_mut(),
        hStdInput: nul_handle,
        hStdOutput: nul_handle,
        hStdError: nul_handle,
    };

    let mut program: Vec<u16> = bin.as_os_str().encode_wide().collect();
    program.push(0);
    // Quote argv[0] exactly like std::process does so spaces in the path are
    // safe. lpApplicationName is set, so CreateProcessW uses it verbatim (no
    // PATH search or extension appending). The command line embeds the program
    // WITHOUT its trailing NUL (only the buffer's final terminator below).
    let mut command_line: Vec<u16> = Vec::with_capacity(program.len() + 16);
    command_line.push(b'"' as u16);
    command_line.extend_from_slice(&program[..program.len() - 1]);
    command_line.push(b'"' as u16);
    command_line.extend(" --daemon".encode_utf16());
    command_line.push(0);

    let mut pi = PROCESS_INFORMATION {
        hProcess: std::ptr::null_mut(),
        hThread: std::ptr::null_mut(),
        dwProcessId: 0,
        dwThreadId: 0,
    };

    let ok = unsafe {
        CreateProcessW(
            program.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // TEMP: bInheritHandles = TRUE (negative test)
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
            std::ptr::null(),
            std::ptr::null(),
            &si as *const STARTUPINFOW,
            &mut pi,
        )
    };
    unsafe {
        let _ = CloseHandle(nul_handle);
    }
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(DaemonChild::Windows(WindowsDaemonProcess {
        process: pi.hProcess,
        thread: pi.hThread,
    }))
}

/// Wait for the gateway to become reachable, spawning a detached daemon if
/// none is running.
///
/// Returns the gateway channel name string, which the caller passes to the
/// muxio IPC client. `bin` defaults to the current executable so tests can
/// point it at `CARGO_BIN_EXE_term-session`.
pub fn connect_or_spawn_server(bin: Option<&std::path::Path>) -> io::Result<String> {
    let gateway = resolve_gateway();
    let socket_name = gateway.to_string();

    if probe_ipc_endpoint(&gateway) {
        return Ok(socket_name);
    }

    let bin = bin
        .map(|b| b.to_path_buf())
        .unwrap_or_else(|| std::env::current_exe().expect("current exe path"));
    let mut child = spawn_detached_server(&bin)?;
    let start = Instant::now();
    let timeout = Duration::from_secs(3);
    let poll_interval = Duration::from_millis(50);

    while start.elapsed() < timeout {
        if probe_ipc_endpoint(&gateway) {
            return Ok(socket_name);
        }
        if let Ok(Some(status)) = child.try_wait() {
            // The spawned daemon died before the socket came up. Another racer
            // may have won the bind; re-probe before surfacing the failure.
            if probe_ipc_endpoint(&gateway) {
                return Ok(socket_name);
            }
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("Gateway exited during startup with status: {status}"),
            ));
        }
        thread::sleep(poll_interval);
    }

    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("Timed out waiting for gateway on channel '{gateway}'"),
    ))
}
