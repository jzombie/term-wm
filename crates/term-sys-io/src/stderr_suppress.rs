//! OS-level stderr suppression for clipboard backends.
//!
//! macOS system frameworks (AppKit, NSPasteboard) and C libraries write debug
//! output directly to FD 2.  During `arboard` clipboard set operations this
//! junk leaks to the terminal.  [`StderrSuppressGuard`] temporarily redirects
//! stderr to the null device and restores it on drop.

use std::sync::{Mutex, MutexGuard};

/// Serialises process-global stderr redirection across threads.
///
/// `StderrSuppressGuard` mutates process-global state by `dup2`ing
/// `STDERR_FILENO`, so concurrent clipboard operations (e.g. a PTY reader
/// thread relaying OSC 52 while the Window Manager copies a selection) would
/// otherwise race on the descriptor.  Every guard construction acquires this
/// lock and holds it until after `Drop` restores the original fd.
static STDERR_MUTEX: Mutex<()> = Mutex::new(());

#[cfg(windows)]
use std::os::windows::io::AsRawHandle;

/// RAII guard that temporarily redirects stderr to the null device.
///
/// Drops restore the original stderr.  Used to suppress transient noise
/// from `arboard` / NSPasteboard during clipboard set operations.
///
/// - **Unix**: opens `/dev/null`, `dup2`s stderr, saves/restores via `dup`.
/// - **Windows**: opens `NUL`, uses `libc::open_osfhandle` + `libc::dup2`
///   for CRT stderr and `SetStdHandle` for the Win32 handle.
///
/// The trailing `_lock` field (dropped after `Drop::drop` runs) holds the
/// process-global `STDERR_MUTEX` until the original fd is restored.
#[cfg(unix)]
pub struct StderrSuppressGuard {
    saved_fd: libc::c_int,
    _lock: MutexGuard<'static, ()>,
}

#[cfg(unix)]
impl StderrSuppressGuard {
    pub fn new() -> Option<Self> {
        let lock = lock_stderr();
        unsafe {
            let null_fd = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
            if null_fd < 0 {
                return None;
            }
            let saved_fd = libc::dup(libc::STDERR_FILENO);
            libc::dup2(null_fd, libc::STDERR_FILENO);
            libc::close(null_fd);
            Some(StderrSuppressGuard {
                saved_fd,
                _lock: lock,
            })
        }
    }
}

#[cfg(unix)]
impl Drop for StderrSuppressGuard {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved_fd, libc::STDERR_FILENO);
            libc::close(self.saved_fd);
        }
    }
}

#[cfg(windows)]
pub struct StderrSuppressGuard {
    saved_handle: isize,
    saved_fd: i32,
    _lock: MutexGuard<'static, ()>,
}

#[cfg(windows)]
impl StderrSuppressGuard {
    pub fn new() -> Option<Self> {
        let lock = lock_stderr();
        unsafe extern "system" {
            fn GetStdHandle(nStdHandle: u32) -> isize;
            fn SetStdHandle(nStdHandle: u32, hHandle: isize) -> i32;
        }
        const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4u32;

        unsafe {
            let saved_handle = GetStdHandle(STD_ERROR_HANDLE);
            let nul = std::fs::OpenOptions::new().write(true).open("NUL").ok()?;
            let nul_handle = nul.as_raw_handle() as isize;
            let nul_fd = libc::open_osfhandle(nul_handle, 0);
            if nul_fd == -1 {
                return None;
            }
            let saved_fd = libc::dup(2);
            libc::dup2(nul_fd, 2);
            SetStdHandle(STD_ERROR_HANDLE, nul_handle);
            Some(StderrSuppressGuard {
                saved_handle,
                saved_fd,
                _lock: lock,
            })
        }
    }
}

#[cfg(windows)]
impl Drop for StderrSuppressGuard {
    fn drop(&mut self) {
        unsafe extern "system" {
            fn SetStdHandle(nStdHandle: u32, hHandle: isize) -> i32;
        }
        const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4u32;
        unsafe {
            libc::dup2(self.saved_fd, 2);
            libc::close(self.saved_fd);
            SetStdHandle(STD_ERROR_HANDLE, self.saved_handle);
        }
    }
}

/// Acquire the process-global stderr mutex, recovering from poisoning.
fn lock_stderr() -> MutexGuard<'static, ()> {
    STDERR_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Fallback: no redirection performed, so no lock is required.
#[cfg(not(any(unix, windows)))]
pub struct StderrSuppressGuard;

#[cfg(not(any(unix, windows)))]
impl StderrSuppressGuard {
    pub fn new() -> Option<Self> {
        Some(StderrSuppressGuard)
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::fd::FromRawFd;
    #[cfg(windows)]
    use std::os::windows::io::FromRawHandle;

    #[test]
    #[cfg(any(unix, windows))]
    fn stderr_suppress_guard_suppresses_and_restores() {
        // ---- platform-specific setup: save + redirect stderr to a pipe ----
        #[cfg(unix)]
        let (capture_fd, restore) = {
            let saved_fd = unsafe { libc::dup(libc::STDERR_FILENO) };
            assert!(saved_fd >= 0, "dup stderr");

            let mut fds: [libc::c_int; 2] = [0; 2];
            unsafe {
                assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            }

            unsafe {
                libc::dup2(fds[1], libc::STDERR_FILENO);
            }
            unsafe {
                libc::close(fds[1]);
            }

            let restore = move || {
                unsafe {
                    libc::dup2(saved_fd, libc::STDERR_FILENO);
                }
                unsafe {
                    libc::close(saved_fd);
                }
            };

            (fds[0] as isize, restore)
        };

        #[cfg(windows)]
        let (capture_fd, restore) = {
            unsafe extern "system" {
                fn GetStdHandle(nStdHandle: u32) -> isize;
                fn SetStdHandle(nStdHandle: u32, hHandle: isize) -> i32;
                fn CreatePipe(
                    hReadPipe: *mut isize,
                    hWritePipe: *mut isize,
                    lpPipeAttributes: *const std::ffi::c_void,
                    nSize: u32,
                ) -> i32;
            }
            const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4u32;

            let saved_handle = unsafe { GetStdHandle(STD_ERROR_HANDLE) };

            // Save the original CRT fd 2 before redirecting.
            let saved_fd2 = unsafe { libc::dup(2) };
            assert!(saved_fd2 >= 0);

            let mut read_handle: isize = 0;
            let mut write_handle: isize = 0;
            unsafe {
                assert_ne!(
                    CreatePipe(&mut read_handle, &mut write_handle, std::ptr::null(), 0),
                    0
                );
            }

            unsafe {
                SetStdHandle(STD_ERROR_HANDLE, write_handle);
            }
            let write_fd = unsafe { libc::open_osfhandle(write_handle, 0) };
            if write_fd != -1 {
                unsafe {
                    libc::dup2(write_fd, 2);
                }
            }

            let restore = move || {
                unsafe {
                    SetStdHandle(STD_ERROR_HANDLE, saved_handle);
                    libc::dup2(saved_fd2, 2);
                    libc::close(saved_fd2);
                }
                if write_fd != -1 {
                    unsafe {
                        libc::close(write_fd);
                    }
                }
            };

            (read_handle, restore)
        };

        // ---- shared assertions ----
        {
            let _guard = StderrSuppressGuard::new();
            assert!(_guard.is_some(), "guard creation");

            #[cfg(unix)]
            unsafe {
                libc::write(libc::STDERR_FILENO, c"suppressed\n".as_ptr().cast(), 11);
            }
            #[cfg(windows)]
            unsafe {
                libc::write(2, c"suppressed\n".as_ptr().cast(), 11);
            }
        }

        #[cfg(unix)]
        unsafe {
            libc::write(libc::STDERR_FILENO, c"restored\n".as_ptr().cast(), 9);
        }
        #[cfg(windows)]
        unsafe {
            libc::write(2, c"restored\n".as_ptr().cast(), 9);
        }

        restore();

        use std::io::Read;
        #[cfg(unix)]
        let mut file = unsafe { std::fs::File::from_raw_fd(capture_fd as _) };
        #[cfg(windows)]
        let mut file = unsafe { std::fs::File::from_raw_handle(capture_fd as _) };
        let mut output = String::new();
        file.read_to_string(&mut output).unwrap_or(0);

        assert!(
            !output.contains("suppressed"),
            "suppressed output leaked to stderr: {output:?}"
        );
        assert!(
            output.contains("restored"),
            "restored output missing from stderr: {output:?}"
        );
    }
}
