use std::fs;
use std::io;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyModifiers};
use ratatui::Frame;
use ratatui::prelude::Rect;
use ratatui::widgets::{Block, Borders};

use term_wm::components::{AsciiImage, Component};
use term_wm::drivers::OutputDriver;
use term_wm::drivers::console::{ConsoleInputDriver, ConsoleOutputDriver};
use term_wm::runner::{HasWindowManager, WindowApp, run_window_app};
use term_wm::window::{AppWindowDraw, WindowManager};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PaneId {
    Left,
    Right,
}

fn main() -> io::Result<()> {
    let mut app = App::new(std::env::args().skip(1).collect())?;
    let mut output = ConsoleOutputDriver::new()?;
    output.enter()?;
    let mut input = ConsoleInputDriver::new();

    let result = run_window_app(
        &mut output,
        &mut input,
        &mut app,
        &[PaneId::Left, PaneId::Right],
        |id| id,
        Some,
        Duration::from_millis(16),
        |event, app| {
            if matches!(event, Event::Mouse(_)) && app.windows.handle_managed_event(event) {
                return true;
            }
            match app.windows.focus() {
                PaneId::Left => app.left.handle_event(event),
                PaneId::Right => app.right.handle_event(event),
            }
        },
        |event, _app| {
            matches!(
                event,
                Some(Event::Key(key))
                    if key.code == KeyCode::Char('q')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
            )
        },
    );

    output.exit()?;

    result
}

struct App {
    windows: WindowManager<PaneId, PaneId>,
    left: AsciiImage,
    right: AsciiImage,
    pending_paths: Vec<String>,
    loaded_count: usize,
}

impl App {
    fn new(mut paths: Vec<String>) -> io::Result<Self> {
        let mut left = AsciiImage::new();
        let mut right = AsciiImage::new();
        left.set_keep_aspect(true);
        right.set_keep_aspect(true);
        left.set_colorize(true);
        right.set_colorize(true);
        if paths.is_empty() {
            paths.push("assets/zenOSmosis-logo.svg".to_string());
        }
        if paths.len() == 1 {
            paths.push(paths[0].clone());
        }
        let mut windows = WindowManager::new_managed(PaneId::Left);
        windows.set_focus_order(vec![PaneId::Left, PaneId::Right]);
        let mut app = Self {
            windows,
            left,
            right,
            pending_paths: paths,
            loaded_count: 0,
        };
        // Initialize windows via the wm_new_window API so creation paths match runtime behavior.
        app.wm_new_window()?;
        app.wm_new_window()?;
        Ok(app)
    }
}

impl HasWindowManager<PaneId, PaneId> for App {
    fn windows(&mut self) -> &mut WindowManager<PaneId, PaneId> {
        &mut self.windows
    }

    fn wm_new_window(&mut self) -> io::Result<()> {
        // Load next pending path into the next available pane (Left then Right).
        if self.loaded_count >= self.pending_paths.len() {
            return Ok(());
        }
        let path = &self.pending_paths[self.loaded_count];
        match self.loaded_count {
            0 => load_into(&mut self.left, path)?,
            1 => load_into(&mut self.right, path)?,
            _ => {}
        }
        self.loaded_count += 1;
        Ok(())
    }
}

impl WindowApp<PaneId, PaneId> for App {
    fn enumerate_windows(&mut self) -> Vec<PaneId> {
        vec![PaneId::Left, PaneId::Right]
    }

    fn render_window(&mut self, frame: &mut Frame, window: AppWindowDraw<PaneId>) {
        match window.id {
            PaneId::Left => {
                render_pane(frame, &mut self.left, window.surface.inner, window.focused)
            }
            PaneId::Right => {
                render_pane(frame, &mut self.right, window.surface.inner, window.focused)
            }
        }
    }

    fn empty_window_message(&self) -> &str {
        "no images loaded"
    }
}

fn render_pane(frame: &mut Frame, image: &mut AsciiImage, area: Rect, _focused: bool) {
    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(area);
    clear_rect(frame, area);
    frame.render_widget(block, area);
    image.render(frame, inner, false);
}

fn clear_rect(frame: &mut Frame, rect: Rect) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let buffer = frame.buffer_mut();
    let bounds = rect.intersection(buffer.area);
    if bounds.width == 0 || bounds.height == 0 {
        return;
    }
    for y in bounds.y..bounds.y.saturating_add(bounds.height) {
        for x in bounds.x..bounds.x.saturating_add(bounds.width) {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.reset();
                cell.set_symbol(" ");
            }
        }
    }
}

fn load_into(component: &mut AsciiImage, path: &str) -> io::Result<()> {
    if path.ends_with(".svg") {
        return component
            .load_svg_from_path(path)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err));
    }
    let bytes = fs::read(path)?;
    let image = decode_pnm(&bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unsupported image"))?;
    match image {
        Pnm::Luma {
            width,
            height,
            data,
        } => component.set_luma8(width, height, data),
        Pnm::Rgba {
            width,
            height,
            data,
        } => component.set_rgba8(width, height, data),
    }
    Ok(())
}

enum Pnm {
    Luma {
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
    Rgba {
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
}

fn decode_pnm(bytes: &[u8]) -> Option<Pnm> {
    let mut idx = 0;
    let magic = next_token(bytes, &mut idx)?;
    let width: u32 = next_token(bytes, &mut idx)?.parse().ok()?;
    let height: u32 = next_token(bytes, &mut idx)?.parse().ok()?;
    let maxval: u32 = next_token(bytes, &mut idx)?.parse().ok()?;
    if maxval == 0 || maxval > 255 {
        return None;
    }
    if magic == "P5" {
        let count = (width * height) as usize;
        let data = bytes.get(idx..idx + count)?.to_vec();
        if maxval != 255 {
            let data = data
                .into_iter()
                .map(|v| ((v as u32 * 255) / maxval) as u8)
                .collect();
            return Some(Pnm::Luma {
                width,
                height,
                data,
            });
        }
        return Some(Pnm::Luma {
            width,
            height,
            data,
        });
    }
    if magic == "P6" {
        let count = (width * height * 3) as usize;
        let raw = bytes.get(idx..idx + count)?.to_vec();
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for chunk in raw.chunks_exact(3) {
            let r = scale_max(chunk[0], maxval);
            let g = scale_max(chunk[1], maxval);
            let b = scale_max(chunk[2], maxval);
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
        return Some(Pnm::Rgba {
            width,
            height,
            data: rgba,
        });
    }
    None
}

fn scale_max(value: u8, maxval: u32) -> u8 {
    if maxval == 255 {
        value
    } else {
        ((value as u32 * 255) / maxval) as u8
    }
}

fn next_token<'a>(bytes: &'a [u8], idx: &mut usize) -> Option<&'a str> {
    while *idx < bytes.len() {
        let b = bytes[*idx];
        if b == b'#' {
            while *idx < bytes.len() && bytes[*idx] != b'\n' {
                *idx += 1;
            }
            continue;
        }
        if b.is_ascii_whitespace() {
            *idx += 1;
            continue;
        }
        break;
    }
    let start = *idx;
    while *idx < bytes.len() && !bytes[*idx].is_ascii_whitespace() {
        *idx += 1;
    }
    std::str::from_utf8(bytes.get(start..*idx)?).ok()
}
