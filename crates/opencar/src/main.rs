//! Terminal lifecycle + the fixed-timestep game loop.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::{cursor, execute, terminal};

use opencar::app::{App, Mode};
use opencar::config::*;
use opencar::display::TermDisplay;

pub fn run(seed: u32) -> io::Result<()> {
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

    let result = drive(&mut stdout, seed, kitty);

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

fn drive(stdout: &mut impl Write, seed: u32, kitty: bool) -> io::Result<()> {
    let mut app = App::new(seed, kitty);
    let mut display = TermDisplay::new();
    let mut backend = opencar::render::create_backend().map_err(io::Error::other)?;
    let mut last = Instant::now();

    loop {
        // Drain input events.
        while event::poll(Duration::from_millis(EVENT_POLL_MILLIS))? {
            match event::read()? {
                Event::Key(key) => app.on_key(&key),
                Event::Resize(..) => display.resize_if_needed(0, 0), // force repaint
                _ => {}
            }
        }

        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;
        app.update(dt);

        if app.mode == Mode::Quit {
            return Ok(());
        }

        let (cols, rows) = terminal::size()?;
        if cols < MIN_CELLS_W || rows < MIN_CELLS_H {
            show_message(stdout, cols, rows)?;
            continue;
        }
        display.resize_if_needed(cols, rows);

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
            let raw = backend.render(&frame);
            let mut owned = raw.to_vec();
            owned.resize(
                cols as usize * rows as usize,
                opencar::braille::TermCell::BLANK,
            );
            app.hud.draw(
                &mut owned,
                cols,
                rows,
                &app.player,
                &app.traffic,
                &app.world,
                backend.name(),
                app.mode == Mode::Paused,
            );
            owned
        };
        display.present(stdout, &cells)?;
    }
}

fn main() -> io::Result<()> {
    // Optional seed as argv[1]; deterministic default otherwise.
    let seed = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(1337);
    run(seed)
}
