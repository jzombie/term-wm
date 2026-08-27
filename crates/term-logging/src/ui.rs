use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;

use term_sys_io::redirect_fd_to_tracing;
use term_wm_core::debug_event_flags::trigger_error_pending;
use term_wm_core::debug_log::{DebugLogWriter, global_debug_log};

use crate::shared::{LOG_FILE_PATH, daemon_filter};

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

/// Inode-aware file writer that follows daemon-owned rotations.
struct InodeAwareFile {
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl InodeAwareFile {
    fn new(path: PathBuf) -> Self {
        Self { path, file: None }
    }

    fn open_file(&self) -> io::Result<std::fs::File> {
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.share_mode(0x1 | 0x2 | 0x4);
        }
        opts.open(&self.path)
    }

    fn needs_reopen(&self) -> bool {
        let Some(file) = self.file.as_ref() else {
            return true;
        };
        let Ok(path_meta) = std::fs::metadata(&self.path) else {
            return true;
        };
        let Ok(file_meta) = file.metadata() else {
            return true;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if path_meta.ino() != file_meta.ino() || path_meta.dev() != file_meta.dev() {
                return true;
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            let path_idx = (path_meta.file_index_high(), path_meta.file_index_low());
            let file_idx = (file_meta.file_index_high(), file_meta.file_index_low());
            if path_idx != file_idx
                || path_meta.volume_serial_number() != file_meta.volume_serial_number()
            {
                return true;
            }
        }
        false
    }

    fn reopen_if_needed(&mut self) -> io::Result<()> {
        if self.needs_reopen() {
            match self.open_file() {
                Ok(f) => self.file = Some(f),
                Err(e) => {
                    self.file = None;
                    return Err(e);
                }
            }
        }
        Ok(())
    }
}

impl Write for InodeAwareFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Err(e) = self.reopen_if_needed() {
            let _ = e;
            return Ok(buf.len());
        }
        let Some(file) = self.file.as_mut() else {
            return Ok(buf.len());
        };
        file.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()
        } else {
            Ok(())
        }
    }
}

/// Initialize tracing for `term-wm` (UI). Mirrors the daemon's `EnvFilter`
/// but tees to the in-app DebugLog and an inode-aware file handle.
pub fn init_ui_logging() {
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(SubscriberMakeWriter)
        .with_target(false)
        .with_thread_names(false)
        .compact();

    let filter = daemon_filter();

    let registry = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(ErrorNotifyLayer)
        .with(filter);

    if let Some(path) = term_wm_config::env::log_file_path() {
        // Publish for panic hook parity with daemon.
        let _ = LOG_FILE_PATH.set(path.clone());
        let writer = InodeAwareFile::new(path);
        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(Mutex::new(writer))
            .with_ansi(false);
        let _ = registry.with(file_layer).try_init();
    } else {
        let _ = registry.try_init();
    }

    #[cfg(unix)]
    {
        let _ = redirect_fd_to_tracing(libc::STDERR_FILENO, true);
    }
    #[cfg(windows)]
    {
        let _ = redirect_fd_to_tracing(2i32, true);
    }
}
