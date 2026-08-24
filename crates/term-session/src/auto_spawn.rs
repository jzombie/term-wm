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
/// Uses the runtime `TERM_WM_GATEWAY` override if present, else the
/// environment-scoped user default (`term-wm/<env>/<user>/gateway`).
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
            DaemonChild::Unix(child) => Ok(child.try_wait()?.map(DaemonExitStatus::Unix)),
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
        use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
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

/// argv flag accepted by both `term-wm` and `term-session` daemon modes to
/// pin the gateway endpoint they bind, bypassing resolution heuristics.
const DAEMON_GATEWAY_ARG: &str = "--gateway";

/// Daemon-mode arguments appended to every detached spawn: run as a daemon,
/// pinned to the exact gateway endpoint this client resolved. Without the
/// pin the fresh child re-runs its own resolution heuristics; any drift
/// between parent and child (CLI-only overrides like `--env`, build
/// heuristics) would bind a different socket and leave the launcher probing
/// a dead name until timeout.
fn daemon_spawn_args(gateway: &str) -> Vec<String> {
    vec![
        "--daemon".to_string(),
        DAEMON_GATEWAY_ARG.to_string(),
        gateway.to_string(),
    ]
}

/// Quoted command-line rendering of [`daemon_spawn_args`] for the Windows
/// `CreateProcessW` command line. Gateway names are validated segments
/// (alphanumeric/hyphen/underscore plus `/` separators), so the defensive
/// quoting never has to escape embedded quotes. Production callers are
/// Windows-only; kept compiling everywhere so the flag/value agreement with
/// [`daemon_spawn_args`] is unit-tested on every platform.
#[cfg_attr(not(windows), allow(dead_code))]
fn daemon_command_line_suffix(gateway: &str) -> String {
    format!(" --daemon {DAEMON_GATEWAY_ARG} \"{gateway}\"")
}

fn spawn_detached_server(bin: &std::path::Path, gateway: &str) -> io::Result<DaemonChild> {
    #[cfg(unix)]
    {
        unix_spawn_detached_server(bin, gateway)
    }
    #[cfg(windows)]
    {
        windows_spawn_detached_server(bin, gateway)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = gateway;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "daemon detachment is not supported on this platform",
        ))
    }
}

#[cfg(unix)]
fn unix_spawn_detached_server(bin: &std::path::Path, gateway: &str) -> io::Result<DaemonChild> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(bin);
    cmd.args(daemon_spawn_args(gateway));
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
fn windows_spawn_detached_server(
    bin: &std::path::Path,
    gateway: &str,
) -> io::Result<DaemonChild> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION,
        STARTF_USESTDHANDLES, STARTUPINFOW,
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
    let mut command_line: Vec<u16> = Vec::with_capacity(program.len() + 32);
    command_line.push(b'"' as u16);
    command_line.extend_from_slice(&program[..program.len() - 1]);
    command_line.push(b'"' as u16);
    // Pin the child to exactly the endpoint this client resolved (mirror of
    // the unix `--gateway` argument; see `daemon_spawn_args`).
    command_line.extend(daemon_command_line_suffix(gateway).encode_utf16());
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
            0, // bInheritHandles = FALSE
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
/// A spawned daemon is pinned to exactly this client's resolved endpoint via
/// `--gateway <name>`, so parent and child can never disagree on the socket
/// (CLI-only overrides like `--env` do not survive the spawn, and heuristic
/// inputs such as `CARGO_MANIFEST_DIR` may differ between the two).
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
    let mut child = spawn_detached_server(&bin, &socket_name)?;
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

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate `TERM_WM_GATEWAY` / `TERM_WM_ENV` /
    /// `USER`, which are process-global and unsafe to read/write concurrently.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn resolve_gateway_honors_override() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var(
                term_wm_config::env::GATEWAY_CHANNEL_ENV_VAR,
                "custom/gateway",
            );
        }
        assert_eq!(resolve_gateway().to_string(), "custom/gateway");
        unsafe {
            std::env::remove_var(term_wm_config::env::GATEWAY_CHANNEL_ENV_VAR);
        }
    }

    #[test]
    fn resolve_gateway_defaults_to_user_scoped_gateway() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var(term_wm_config::env::GATEWAY_CHANNEL_ENV_VAR);
            std::env::remove_var(term_wm_config::NAMESPACE_ENV_VAR);
            // `current_os_user()` reads $USER on Unix and %USERNAME% on
            // Windows; set both so the assertion is platform-independent.
            std::env::set_var("USER", "tester");
            std::env::set_var("USERNAME", "tester");
        }
        let gw = resolve_gateway();
        // Static default: {namespace}/<user>/gateway. No environment
        // component by design; both override variables are cleared so the
        // assertion holds regardless of ambient toolchain injection.
        assert_eq!(
            gw.to_string(),
            format!("{}/tester/gateway", term_wm_config::GATEWAY_NAMESPACE)
        );
        assert_eq!(gw.namespace, term_wm_config::GATEWAY_NAMESPACE);
        unsafe {
            std::env::remove_var("USER");
            std::env::remove_var("USERNAME");
        }
    }

    #[test]
    fn resolve_gateway_namespace_override_preserves_user_segment() {
        let _guard = env_lock();
        // The toolchain-injected namespace override replaces only the root
        // segment: OS-level user isolation survives on shared dev machines.
        unsafe {
            std::env::remove_var(term_wm_config::env::GATEWAY_CHANNEL_ENV_VAR);
            std::env::set_var(term_wm_config::NAMESPACE_ENV_VAR, "term-wm-dev");
            std::env::set_var("USER", "tester");
            std::env::set_var("USERNAME", "tester");
        }
        let gw = resolve_gateway();
        assert_eq!(gw.to_string(), "term-wm-dev/tester/gateway");
        unsafe {
            std::env::remove_var(term_wm_config::NAMESPACE_ENV_VAR);
            std::env::remove_var("USER");
            std::env::remove_var("USERNAME");
        }
    }

    #[test]
    fn resolve_gateway_keeps_multi_segment_overrides_lossless() {
        let _guard = env_lock();
        // Full endpoint paths round-trip byte-exact (daemon spawn pinning
        // depends on this); they must not collapse to a shorter name.
        let raw = "term-wm-dev-1a2b3c4d/prod/alice/gateway";
        unsafe {
            std::env::set_var(term_wm_config::env::GATEWAY_CHANNEL_ENV_VAR, raw);
        }
        assert_eq!(resolve_gateway().to_string(), raw);
        unsafe {
            std::env::remove_var(term_wm_config::env::GATEWAY_CHANNEL_ENV_VAR);
        }
    }

    #[test]
    fn resolve_gateway_falls_back_when_override_is_malformed() {
        let _guard = env_lock();
        // Invalid segments cannot parse as a gateway endpoint; the fallback
        // is the legacy `{namespace}/gateway` name.
        unsafe {
            std::env::set_var(term_wm_config::env::GATEWAY_CHANNEL_ENV_VAR, "has space/gateway");
        }
        assert_eq!(
            resolve_gateway().to_string(),
            format!("{}/gateway", term_wm_config::GATEWAY_NAMESPACE)
        );
        unsafe {
            std::env::remove_var(term_wm_config::env::GATEWAY_CHANNEL_ENV_VAR);
        }
    }

    #[cfg(unix)]
    #[test]
    fn daemon_exit_status_formats_unix_status() {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 3")
            .status()
            .expect("run sh");
        let rendered = DaemonExitStatus::Unix(status).to_string();
        assert!(rendered.contains("exit status"), "rendered: {rendered}");
    }

    #[test]
    fn daemon_spawn_args_pin_the_resolved_gateway() {
        let args = daemon_spawn_args("term-wm-dev-1a2b3c4d/prod/alice/gateway");
        assert_eq!(args[0], "--daemon");
        assert_eq!(args[1], DAEMON_GATEWAY_ARG);
        assert_eq!(
            args[2], "term-wm-dev-1a2b3c4d/prod/alice/gateway",
            "the pinned name must be the full multi-segment endpoint"
        );
    }

    #[test]
    fn daemon_command_line_suffix_agrees_with_spawn_args() {
        // Windows builds one quoted command line; it must carry the same
        // flag/value pair the unix argv path passes.
        let suffix = daemon_command_line_suffix("term-wm/prod/alice/gateway");
        assert_eq!(suffix, format!(" --daemon {DAEMON_GATEWAY_ARG} \"term-wm/prod/alice/gateway\""));
        assert!(suffix.contains(DAEMON_GATEWAY_ARG));
    }
}
