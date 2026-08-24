use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{Layer, layer::Context};

use term_sys_io::redirect_fd_to_tracing;
use term_wm_config::env::LOG_FILE_ENV_VAR;
use term_wm_core::debug_event_flags::trigger_error_pending;

use term_wm_core::debug_log::{DebugLogWriter, global_debug_log};

struct ErrorNotifyLayer;

impl<S> Layer<S> for ErrorNotifyLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() == Level::ERROR {
            trigger_error_pending();
        }
    }
}

/// The durable log destination resolved from [`LOG_FILE_ENV_VAR`], opened
/// once (append mode) at subscriber init and shared by every writer clone.
static LOG_FILE: OnceLock<Option<Arc<Mutex<std::fs::File>>>> = OnceLock::new();

fn log_file_slot() -> &'static Option<Arc<Mutex<std::fs::File>>> {
    LOG_FILE.get_or_init(|| {
        term_wm_config::env::log_file_path().and_then(|path| open_log_file(&path))
    })
}

/// Open the log file for appending (create-if-missing), mirroring the
/// `TERM_WM_TRACE_ESC` convention. Failure to open is non-fatal: logging
/// falls back to the in-app Debug Log / stderr only.
fn open_log_file(path: &PathBuf) -> Option<Arc<Mutex<std::fs::File>>> {
    match std::fs::OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => Some(Arc::new(Mutex::new(file))),
        Err(e) => {
            // No subscriber exists yet at init time; report on real stderr so
            // the misconfiguration is visible somewhere.
            eprintln!("warning: cannot open {LOG_FILE_ENV_VAR} target {path:?}: {e}");
            None
        }
    }
}

pub struct DelegatingWriter {
    inner: DelegatingInner,
}

enum DelegatingInner {
    Debug(DebugLogWriter),
    Stderr(io::Stderr),
}

impl DelegatingWriter {
    fn new() -> Self {
        if let Some(handle) = global_debug_log() {
            return DelegatingWriter {
                inner: DelegatingInner::Debug(handle.writer()),
            };
        }
        DelegatingWriter {
            inner: DelegatingInner::Stderr(io::stderr()),
        }
    }
}

impl Write for DelegatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Tee into the configured log file when one is set (#270): the
        // ring-buffer window is lost on exit, the file is not.
        if let Some(file) = log_file_slot()
            && let Ok(mut file) = file.lock()
        {
            let _ = file.write_all(buf);
        }
        match &mut self.inner {
            DelegatingInner::Debug(w) => w.write(buf),
            DelegatingInner::Stderr(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = log_file_slot()
            && let Ok(mut file) = file.lock()
        {
            let _ = file.flush();
        }
        match &mut self.inner {
            DelegatingInner::Debug(w) => w.flush(),
            DelegatingInner::Stderr(s) => s.flush(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SubscriberMakeWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SubscriberMakeWriter {
    type Writer = DelegatingWriter;

    fn make_writer(&'a self) -> Self::Writer {
        DelegatingWriter::new()
    }
}

/// Initialize tracing and redirect stderr into it.
///
/// Routes tracing output to the in-app Debug Log window when available
/// (falls back to stderr).  Also redirects the OS-level stderr FD into
/// tracing so framework noise (NSPasteboard, etc.) goes to the log
/// instead of the terminal.  Safe to call multiple times.
///
/// When `TERM_WM_LOG_FILE` is set, every event additionally tees to that
/// file (append mode), giving crashes/hangs a durable diagnostic trail.
pub fn init_default() {
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(SubscriberMakeWriter)
        .with_target(false)
        .with_thread_names(false)
        .compact();

    let _ = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(ErrorNotifyLayer)
        .with(tracing_subscriber::filter::LevelFilter::from_level(
            Level::DEBUG,
        ))
        .try_init();

    // Redirect stderr into tracing so system-framework debug output
    // (NSPasteboard, etc.) goes to the debug log instead of the terminal.
    // stdout is NOT redirected — ratatui/crossterm render to stdout.
    #[cfg(unix)]
    {
        let _ = redirect_fd_to_tracing(libc::STDERR_FILENO, true);
    }
    #[cfg(windows)]
    {
        let _ = redirect_fd_to_tracing(2i32, true);
    }
}
