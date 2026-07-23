use std::io::{self, Write};

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{Layer, layer::Context};

use term_wm_core::debug_event_flags::trigger_error_pending;
use term_wm_pty_engine::redirect_stdio::redirect_fd_to_tracing;

#[cfg(feature = "sys-ui")]
use term_wm_sys_ui_components::wm_debug_log::{DebugLogWriter, global_debug_log};

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

pub struct DelegatingWriter {
    inner: DelegatingInner,
}

enum DelegatingInner {
    #[cfg(feature = "sys-ui")]
    Debug(DebugLogWriter),
    Stderr(io::Stderr),
}

impl DelegatingWriter {
    fn new() -> Self {
        #[cfg(feature = "sys-ui")]
        {
            if let Some(handle) = global_debug_log() {
                return DelegatingWriter {
                    inner: DelegatingInner::Debug(handle.writer()),
                };
            }
        }
        DelegatingWriter {
            inner: DelegatingInner::Stderr(io::stderr()),
        }
    }
}

impl Write for DelegatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.inner {
            #[cfg(feature = "sys-ui")]
            DelegatingInner::Debug(w) => w.write(buf),
            DelegatingInner::Stderr(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.inner {
            #[cfg(feature = "sys-ui")]
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

    // This is safe.  It should show up in the debug console.
    eprintln!("stderr redirected");
}
