//! TerminalComponent render benchmark — guards against O(rows*cols*row) regressions.
//!
//! Run with:
//!   cargo bench -p term-wm-ui-components --bench terminal_render
//! or all benches:
//!   cargo bench
//!
//! First run is slow (criterion warmup). Subsequent runs compare against
//! `target/criterion/` baseline. If 320×100 is ~16× slower than 80×24
//! instead of ~4×, the `visible_row()` hoist has regressed.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use portable_pty::PtySize;
use std::sync::{Arc, Mutex};
use term_wm_core::components::{Component, ComponentContext};
use term_wm_core::hitbox_registry::HitboxRegistry;
use term_wm_layout_engine::LayoutRect;
use term_wm_pty_engine::{Pane, PtyResult};
use term_wm_ui_components::TerminalComponent;

/// Lightweight mock Pane for benchmarking without launching a sub-process.
struct BenchPane {
    parser: Arc<Mutex<term_wm_vt100::Parser>>,
}

impl BenchPane {
    fn new(rows: u16, cols: u16) -> Self {
        let mut parser = term_wm_vt100::Parser::new(rows, cols, 1000);
        // Fill the screen with characters so rows and cells exist in the VT100 grid
        let line = "X".repeat(cols as usize) + "\r\n";
        for _ in 0..rows {
            parser.process(line.as_bytes());
        }
        Self {
            parser: Arc::new(Mutex::new(parser)),
        }
    }
}

impl Pane for BenchPane {
    fn resize(&mut self, _size: PtySize) -> PtyResult<()> {
        Ok(())
    }
    fn has_exited(&mut self) -> bool {
        false
    }
    fn alternate_screen(&mut self) -> bool {
        false
    }
    fn scrollback(&mut self) -> usize {
        0
    }
    fn set_scrollback(&mut self, _rows: usize) {}
    fn write_bytes(&mut self, _input: &[u8]) -> std::io::Result<()> {
        Ok(())
    }
    fn shared_parser(&mut self) -> Arc<Mutex<term_wm_vt100::Parser>> {
        self.parser.clone()
    }
    fn max_scrollback(&mut self) -> usize {
        0
    }
    fn scrollback_len(&self) -> usize {
        0
    }
    fn take_exit_status(&mut self) -> Option<portable_pty::ExitStatus> {
        None
    }
    fn exit_status(&self) -> Option<portable_pty::ExitStatus> {
        None
    }
    fn bytes_received(&self) -> usize {
        100
    }
    fn last_bytes_text(&self) -> String {
        String::new()
    }
    fn kill_child(&mut self) -> PtyResult<()> {
        Ok(())
    }
    fn take_pending_title(&mut self) -> Option<String> {
        None
    }
}

fn bench_terminal_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("terminal_render_screen");

    for (cols, rows) in [(80, 24), (160, 50), (320, 100)] {
        group.bench_with_input(
            BenchmarkId::new("grid_size", format!("{}x{}", cols, rows)),
            &(cols, rows),
            |b, &(cols, rows)| {
                let bench_pane = BenchPane::new(rows, cols);
                let mut term = TerminalComponent::from_pane(Box::new(bench_pane));

                let layout_area = LayoutRect {
                    x: 0,
                    y: 0,
                    width: cols,
                    height: rows,
                };
                let rect = ratatui::layout::Rect::new(0, 0, cols, rows);
                let ctx = ComponentContext::new(true);

                b.iter(|| {
                    let buffer = ratatui::buffer::Buffer::empty(rect);
                    let mut backend = term_wm_console::RatatuiBackend::new_simple(buffer, rect);
                    let mut registry = HitboxRegistry::new();

                    // Calls TerminalComponent::render(), which executes term-wm's
                    // render_screen(), VT100 visible_row() lookups, cell style resolution,
                    // and link overlay logic.
                    term.render(&mut backend, layout_area, &ctx, &mut registry);

                    std::hint::black_box(backend);
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_terminal_render);
criterion_main!(benches);
