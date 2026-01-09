use std::io::{self, Stdout};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use crossterm::{
    cursor,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Paragraph,
};

const GLYPHS: [&str; 10] = [".", ",", ":", "-", ";", "+", "*", "x", "#", "@"];

#[derive(Parser, Debug)]
#[command(
    name = "render-bench",
    version = env!("CARGO_PKG_VERSION"),
    about = "Render-heavy benchmark for checking terminal throughput"
)]
struct BenchCli {
    /// How long to run the benchmark.
    #[arg(
        short = 'd',
        long = "duration",
        value_name = "SECONDS",
        default_value_t = 10.0
    )]
    duration_seconds: f64,

    /// Target frames per second. Used to pace rendering so comparisons are repeatable.
    #[arg(short = 'f', long = "fps", value_name = "FPS", default_value_t = 60.0)]
    target_fps: f64,
}

impl BenchCli {
    fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.duration_seconds)
    }

    fn frame_budget(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.target_fps)
    }
}

struct BenchConfig {
    duration: Duration,
    target_fps: f64,
    frame_budget: Duration,
}

impl TryFrom<&BenchCli> for BenchConfig {
    type Error = String;

    fn try_from(cli: &BenchCli) -> Result<Self, Self::Error> {
        if !(0.5..=600.0).contains(&cli.duration_seconds) {
            return Err("duration must be between 0.5 and 600 seconds".to_string());
        }
        if !(1.0..=240.0).contains(&cli.target_fps) {
            return Err("fps must be between 1 and 240".to_string());
        }
        Ok(Self {
            duration: cli.duration(),
            target_fps: cli.target_fps,
            frame_budget: cli.frame_budget(),
        })
    }
}

fn main() -> io::Result<()> {
    let args = BenchCli::parse();
    let config = BenchConfig::try_from(&args)
        .map_err(|msg| io::Error::new(io::ErrorKind::InvalidInput, msg))?;

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        cursor::Hide
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let bench_result = run_benchmark(&mut terminal, &config);

    terminal.show_cursor()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        cursor::Show
    )?;
    terminal::disable_raw_mode()?;

    let stats = bench_result?;
    println!("{}", stats.final_report(&config));

    Ok(())
}

type BenchTerminal = Terminal<CrosstermBackend<Stdout>>;

fn run_benchmark(terminal: &mut BenchTerminal, config: &BenchConfig) -> io::Result<BenchStats> {
    let mut stats = BenchStats::new();
    let mut noise = NoiseField::seeded_from_clock();
    let mut tick: u64 = 0;
    let mut exit_reason = ExitReason::Completed;

    loop {
        let frame_start = Instant::now();
        let mut cells_drawn: u64 = 0;
        terminal.draw(|frame| {
            cells_drawn = draw_frame(frame, tick, &stats, &mut noise, config);
        })?;
        let draw_time = frame_start.elapsed();
        stats.record_frame(cells_drawn, draw_time);

        if stats.elapsed() >= config.duration {
            break;
        }

        if poll_for_exit(config.frame_budget.saturating_sub(draw_time))? {
            exit_reason = ExitReason::UserAbort;
            break;
        }

        tick = tick.wrapping_add(1);
    }

    stats.exit_reason = exit_reason;
    stats.mark_completed();
    Ok(stats)
}

fn draw_frame(
    frame: &mut Frame,
    tick: u64,
    stats: &BenchStats,
    noise: &mut NoiseField,
    config: &BenchConfig,
) -> u64 {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return 0;
    }

    let overlay_lines = build_overlay_lines(stats, config);
    let overlay_info = OverlayState::new(area, &overlay_lines);

    {
        let buffer = frame.buffer_mut();
        noise.fill(buffer, area, tick);
        if let Some(overlay_area) = overlay_info.area {
            fill_rect(buffer, overlay_area, Style::default().bg(Color::Black));
        }
    }

    if let Some(overlay_area) = overlay_info.area {
        frame.render_widget(
            Paragraph::new(overlay_lines.join("\n"))
                .style(Style::default().fg(Color::White).bg(Color::Black)),
            overlay_area,
        );
    }

    area.width as u64 * area.height as u64
}

fn fill_rect(buffer: &mut Buffer, area: Rect, style: Style) {
    for y in 0..area.height {
        for x in 0..area.width {
            let px = area.x.saturating_add(x);
            let py = area.y.saturating_add(y);
            buffer[(px, py)].set_symbol(" ").set_style(style);
        }
    }
}

fn build_overlay_lines(stats: &BenchStats, config: &BenchConfig) -> Vec<String> {
    let elapsed = stats.elapsed().as_secs_f64();
    let duration_target = config.duration.as_secs_f64();
    let progress = if duration_target > 0.0 {
        (elapsed / duration_target).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let fps_avg = if elapsed > 0.0 {
        stats.frame_count as f64 / elapsed
    } else {
        0.0
    };
    let avg_ms = stats.average_frame_ms();
    let best = stats.fastest_frame_ms();
    let worst = stats.slowest_frame_ms();
    let updates_per_sec = if elapsed > 0.0 {
        stats.cell_updates as f64 / elapsed
    } else {
        0.0
    };

    vec![
        "== Render Bench ==".to_string(),
        format!(
            "elapsed {:>5.1}/{:>5.1}s ({:>3.0}%)",
            elapsed,
            duration_target,
            progress * 100.0
        ),
        format!(
            "frames {:>8} | avg fps {:>5.1} / target {:>5.1}",
            stats.frame_count, fps_avg, config.target_fps
        ),
        format!(
            "cells {:>11} | {:>8.0}/s",
            stats.cell_updates, updates_per_sec
        ),
        format!(
            "frame ms avg {:>6.2} | best {:>5.2} | worst {:>5.2}",
            avg_ms, best, worst
        ),
        format!("exit: {}", stats.exit_reason.describe()),
        "press q / esc / ctrl+c to stop".to_string(),
    ]
}

struct OverlayState {
    area: Option<Rect>,
}

impl OverlayState {
    fn new(window_area: Rect, lines: &[String]) -> Self {
        let available_width = window_area.width.saturating_sub(2);
        let available_height = window_area.height.saturating_sub(2);
        if available_width < 8 || available_height < 4 {
            return Self { area: None };
        }
        let text_width = lines
            .iter()
            .map(|line| line.len() as u16)
            .max()
            .unwrap_or(0);
        let text_height = lines.len() as u16;
        let width = text_width.saturating_add(2).clamp(8, available_width);
        let height = text_height.saturating_add(2).clamp(4, available_height);
        let rect = Rect {
            x: window_area.x + 1,
            y: window_area.y + 1,
            width,
            height,
        };
        Self { area: Some(rect) }
    }
}

struct BenchStats {
    start: Instant,
    completed_at: Option<Instant>,
    frame_count: u64,
    cell_updates: u64,
    total_draw_time: Duration,
    fastest_frame: Duration,
    slowest_frame: Duration,
    exit_reason: ExitReason,
}

impl BenchStats {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            completed_at: None,
            frame_count: 0,
            cell_updates: 0,
            total_draw_time: Duration::ZERO,
            fastest_frame: Duration::MAX,
            slowest_frame: Duration::ZERO,
            exit_reason: ExitReason::Completed,
        }
    }

    fn elapsed(&self) -> Duration {
        match self.completed_at {
            Some(done) => done.duration_since(self.start),
            None => self.start.elapsed(),
        }
    }

    fn mark_completed(&mut self) {
        self.completed_at = Some(Instant::now());
    }

    fn record_frame(&mut self, cells: u64, draw_time: Duration) {
        self.frame_count = self.frame_count.saturating_add(1);
        self.cell_updates = self.cell_updates.saturating_add(cells);
        self.total_draw_time += draw_time;
        if draw_time < self.fastest_frame {
            self.fastest_frame = draw_time;
        }
        if draw_time > self.slowest_frame {
            self.slowest_frame = draw_time;
        }
    }

    fn average_frame_ms(&self) -> f64 {
        if self.frame_count == 0 {
            return 0.0;
        }
        (self.total_draw_time.as_secs_f64() / self.frame_count as f64) * 1_000.0
    }

    fn fastest_frame_ms(&self) -> f64 {
        if self.frame_count == 0 {
            return 0.0;
        }
        self.fastest_frame.as_secs_f64() * 1_000.0
    }

    fn slowest_frame_ms(&self) -> f64 {
        if self.frame_count == 0 {
            return 0.0;
        }
        self.slowest_frame.as_secs_f64() * 1_000.0
    }

    fn final_report(&self, config: &BenchConfig) -> String {
        let elapsed = self.elapsed().as_secs_f64();
        let fps_avg = if elapsed > 0.0 {
            self.frame_count as f64 / elapsed
        } else {
            0.0
        };
        let cells_per_second = if elapsed > 0.0 {
            self.cell_updates as f64 / elapsed
        } else {
            0.0
        };

        indoc::formatdoc!(
            r#"
            Render bench {status}.
            Duration: {elapsed:.2}s (target {target:.2}s)
            Frames: {frames} | Avg FPS: {fps:.1} (target {target_fps:.1})
            Avg frame: {avg:.2} ms | Best: {best:.2} ms | Worst: {worst:.2} ms
            Cell updates: {cells} total (~{cells_per_sec:.0}/s)
            "#,
            status = self.exit_reason.describe(),
            elapsed = elapsed,
            target = config.duration.as_secs_f64(),
            frames = self.frame_count,
            fps = fps_avg,
            target_fps = config.target_fps,
            avg = self.average_frame_ms(),
            best = self.fastest_frame_ms(),
            worst = self.slowest_frame_ms(),
            cells = self.cell_updates,
            cells_per_sec = cells_per_second,
        )
    }
}

#[derive(Copy, Clone)]
enum ExitReason {
    Completed,
    UserAbort,
}

impl ExitReason {
    fn describe(self) -> &'static str {
        match self {
            ExitReason::Completed => "completed full duration",
            ExitReason::UserAbort => "stopped by user",
        }
    }
}

struct NoiseField {
    state: u64,
}

impl NoiseField {
    fn seeded_from_clock() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ 0xA5A5_A5A5_1234_5678;
        Self { state: seed }
    }

    fn next(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (self.state >> 32) as u32
    }

    fn fill(&mut self, buffer: &mut Buffer, area: Rect, tick: u64) {
        for y in 0..area.height {
            for x in 0..area.width {
                let glyph_idx = (self.next() as usize) % GLYPHS.len();
                let glyph = GLYPHS[glyph_idx];
                let base = ((x as u32 * 5 + y as u32 * 3 + tick as u32) & 0xFF) as u8;
                let color = Color::Rgb(
                    base,
                    base.wrapping_add(((tick >> 1) as u8).wrapping_mul(3)),
                    base.wrapping_add(((tick >> 2) as u8).wrapping_mul(5)),
                );
                let modifier = if (self.next() & 0x2) == 0 {
                    Modifier::empty()
                } else {
                    Modifier::BOLD
                };
                let px = area.x.saturating_add(x);
                let py = area.y.saturating_add(y);
                buffer[(px, py)].set_symbol(glyph).set_style(
                    Style::default()
                        .fg(color)
                        .bg(Color::Black)
                        .add_modifier(modifier),
                );
            }
        }
    }
}

fn poll_for_exit(wait: Duration) -> io::Result<bool> {
    if !event::poll(wait)? {
        return Ok(false);
    }
    loop {
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if matches!(
                    key.code,
                    KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc
                ) {
                    return Ok(true);
                }
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Ok(true);
                }
            }
            _ => {}
        }
        if !event::poll(Duration::ZERO)? {
            break;
        }
    }
    Ok(false)
}
