use std::io::{self, Write};

use tracing::Level;

use crate::components::debug_log::{DebugLogWriter, global_debug_log};

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
            DelegatingWriter {
                inner: DelegatingInner::Debug(handle.writer()),
            }
        } else {
            DelegatingWriter {
                inner: DelegatingInner::Stderr(io::stderr()),
            }
        }
    }
}

impl Write for DelegatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.inner {
            DelegatingInner::Debug(w) => w.write(buf),
            DelegatingInner::Stderr(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
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

/// Initialize tracing subscriber to write to the debug log buffer when available,
/// otherwise fall back to stderr. Safe to call multiple times; subsequent calls
/// are no-ops for the global subscriber.
pub fn init_default() {
    // Configure a compact formatter and delegate writes to our make-writer.
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_writer(SubscriberMakeWriter)
        .with_target(false)
        .with_thread_names(false)
        .try_init();
}
