//! Win32 Job Object containment for PTY process trees (Windows only).
//!
//! A spawned PTY child is assigned to a Job Object configured with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Every descendant process inherits
//! membership in the job, so terminating the job (`TerminateJobObject`) or
//! closing the last job handle kills the **entire tree** — including
//! grandchildren — rather than only the session leader. This is the Windows
//! analogue of Unix `kill(-pgid, signal)` and keeps background jobs from being
//! orphaned when a session is killed.

use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// Owned handle to a Win32 Job Object configured to terminate its process
/// tree when the last handle is closed.
pub struct JobObject {
    handle: HANDLE,
}

impl JobObject {
    /// Create an anonymous job with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    ///
    /// The kill-on-close flag is set **before** any process is assigned, so
    /// the containment guarantee holds even if this `JobObject` is dropped
    /// without an explicit `terminate()`.
    pub fn new() -> io::Result<Self> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let err = io::Error::last_os_error();
                CloseHandle(handle);
                return Err(err);
            }
            Ok(Self { handle })
        }
    }

    /// Assign an already-running process (by its handle) to the job.
    ///
    /// `process` must be a valid, open handle to the process (e.g. the PTY
    /// child's handle from `Child::as_raw_handle`). Since Windows 8 processes
    /// may belong to multiple (nested) jobs, this succeeds even when the
    /// process was spawned from a parent that is itself inside a job. A
    /// failure means containment could not be established; the caller falls
    /// back to single-process termination.
    ///
    /// # Safety
    /// `process` must reference a live process handle for the duration of the
    /// call.
    pub unsafe fn assign(&self, process: HANDLE) -> io::Result<()> {
        let ok = unsafe { AssignProcessToJobObject(self.handle, process) };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Terminate every process currently in the job (the whole tree).
    pub fn terminate(&self, exit_code: u32) -> io::Result<()> {
        let ok = unsafe { TerminateJobObject(self.handle, exit_code) };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

// Job Object handles are thread-safe: `TerminateJobObject` and
// `AssignProcessToJobObject` may be called from any thread, and the OS owns
// the underlying object's lifetime. `HANDLE` is a pointer-sized value with no
// thread-affinity semantics, so moving/cloning it across threads is sound.
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl Drop for JobObject {
    fn drop(&mut self) {
        // Closing the last job handle fires `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`,
        // terminating any processes still in the job. Best-effort: a failure
        // only means the tree was already gone.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}
