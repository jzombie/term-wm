//! Terminal lifecycle + the fixed-timestep game loop.

use std::io::{self, BufWriter, Write};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::{cursor, execute, terminal};

use opencar::app::{App, Mode};
use opencar::config::*;
use opencar::display::TermDisplay;

pub fn run(seed: u32, debug_frame: Option<usize>, capture_out: Option<String>) -> io::Result<()> {
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;

    // Kitty keyboard protocol: real Press/Repeat/Release when supported.
    let kitty = terminal::supports_keyboard_enhancement().unwrap_or(false);
    if kitty {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        )?;
    }

    let result = if let Some(path) = capture_out {
        let file = std::fs::File::create(&path)?;
        let mut tee = TeeWriter {
            out: &mut stdout,
            cap: file,
        };
        drive(&mut tee, seed, kitty, debug_frame)
    } else {
        drive(&mut stdout, seed, kitty, debug_frame)
    };

    // Teardown runs no matter how the loop ended.
    if kitty {
        execute!(stdout, PopKeyboardEnhancementFlags)?;
    }
    execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    result
}

fn show_message(stdout: &mut impl Write, cols: u16, rows: u16) -> io::Result<()> {
    let msg = "opencar needs a bigger terminal (min 48x16)";
    let row = rows / 2;
    let col = cols.saturating_sub(msg.len() as u16) / 2;
    write!(
        stdout,
        "\x1b[2J\x1b[{};{}H{}",
        row.saturating_add(1),
        col.saturating_add(1),
        msg
    )?;
    stdout.flush()
}

/// Tees display output into a capture file while still writing to stdout.
struct TeeWriter<'a> {
    out: &'a mut dyn Write,
    cap: std::fs::File,
}
impl Write for TeeWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.cap.write_all(buf)?;
        self.out.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.cap.flush()?;
        self.out.flush()
    }
}

fn drive(
    stdout: &mut impl Write,
    seed: u32,
    kitty: bool,
    debug_frame: Option<usize>,
) -> io::Result<()> {
    let mut app = App::new(seed, kitty);
    let mut frame_n = 0usize;
    let mut display = TermDisplay::new();
    let mut backend = opencar::render::create_backend().map_err(io::Error::other)?;
    let mut last = Instant::now();

    // F1: one big buffered writer for the whole session — collapses the
    // escape stream into a few large writes per frame. Owned by this scope,
    // so the underlying stdout lock is released before run() performs
    // crossterm teardown.
    let mut out = BufWriter::with_capacity(OUT_BUF_BYTES, stdout);

    // F3 draw gate: cadence anchored to each draw's start; overruns leave
    // next_draw in the past ⇒ immediate redraw (natural PTY backpressure).
    let target_frame_time = Duration::from_micros(16_667);
    let mut next_draw = Instant::now();

    loop {
        let frame_start = Instant::now();

        // Drain input events (also the idle yield between draws).
        while event::poll(Duration::from_millis(EVENT_POLL_MILLIS))? {
            match event::read()? {
                Event::Key(key) => app.on_key(&key),
                Event::FocusLost => app.on_focus_lost(),
                Event::Resize(..) => display.resize_if_needed(0, 0), // force repaint
                _ => {}
            }
        }

        let dt = frame_start.duration_since(last).as_secs_f32();
        last = frame_start;
        let update_start = Instant::now();
        app.update(dt);
        let update_ms = update_start.elapsed().as_secs_f32() * 1000.0;

        if app.mode == Mode::Quit {
            return Ok(());
        }

        let (cols, rows) = terminal::size()?;
        if cols < MIN_CELLS_W || rows < MIN_CELLS_H {
            show_message(&mut out, cols, rows)?;
            continue;
        }
        display.resize_if_needed(cols, rows);

        // Draw gate: skip render+present entirely while ahead of cadence;
        // input and physics above keep ticking regardless.
        let draw_started_at = Instant::now();
        if draw_started_at < next_draw {
            continue;
        }
        next_draw = draw_started_at + target_frame_time;

        let cells = {
            let frame = opencar::render::FrameInput {
                world: &app.world,
                cam: &app.cam,
                player: &app.player,
                traffic: &app.traffic,
                env: &app.env,
                cells_w: cols,
                cells_h: rows,
            };
            let render_start = Instant::now();
            let raw = backend.render(&frame);
            let mut owned = raw.to_vec();
            owned.resize(
                cols as usize * rows as usize,
                opencar::braille::TermCell::BLANK,
            );
            // Frame-stability signal: FNV-1a of the rendered grid BEFORE HUD.
            let fh = opencar::hud::frame_hash(&owned);
            app.hud.draw(
                &mut owned,
                cols,
                rows,
                &app.player,
                &app.traffic,
                &app.world,
                backend.name(),
                app.mode == Mode::Paused,
                Some(fh.as_str()),
            );
            app.hud
                .perf
                .push(update_ms, render_start.elapsed().as_secs_f32() * 1000.0, 0.0);
            owned
        };

        let io_start = Instant::now();
        display.present(&mut out, &cells)?;
        app.hud.perf.set_io(io_start.elapsed().as_secs_f32() * 1000.0);

        // ── Diagnostics: synchronous dump + clean exit on --debug-frame=N ──
        frame_n += 1;
        if debug_frame == Some(frame_n) {
            backend.dump_to(&cells, cols as usize, std::path::Path::new("/tmp"))?;
            eprintln!("opencar: dumped frame {frame_n} to /tmp");
            return Ok(());
        }

        // ── Diagnostics (K key): detached writer thread ──
        if app.dump_request {
            app.dump_request = false;
            let (rgb, w, h) = backend.frame_snapshot();
            let cols_usize = cols as usize;
            let cells_clone = cells.clone();
            std::thread::spawn(move || {
                use opencar::render::image::{ImageBuffer, write_cells_txt};
                let img = ImageBuffer {
                    w,
                    h,
                    rgb,
                    z: Vec::new(),
                };
                let dir = std::path::Path::new("/tmp");
                let _ = img.write_ppm(&dir.join("frame_rgb.ppm"));
                let _ = write_cells_txt(&cells_clone, cols_usize, &dir.join("frame_cells.txt"));
            });
        }
    }
}

fn main() -> io::Result<()> {
    let mut seed = {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| (d.as_nanos() as u64 ^ 0x9E3779B97F4A7C15) as u32)
            .unwrap_or(1337)
    };
    let mut debug_frame = None;
    let mut capture_out = None;
    for a in std::env::args().skip(1) {
        if let Some(v) = a.strip_prefix("--debug-frame=") {
            debug_frame = v.parse::<usize>().ok();
        } else if let Some(v) = a.strip_prefix("--capture-out=") {
            capture_out = Some(v.to_string());
        } else if let Ok(n) = a.parse::<u32>() {
            seed = n;
        }
    }
    run(seed, debug_frame, capture_out)
}
