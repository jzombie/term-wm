use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

// The blit_buffer function is pub in draw_plan_renderer.
// We import from the crate root.
use term_wm_console::draw_plan_renderer::blit_buffer;

fn bench_blit_buffer(c: &mut Criterion) {
    let sizes = [
        ("80x24 fully contained", Rect::new(0, 0, 80, 24)),
        ("120x40", Rect::new(0, 0, 120, 40)),
        ("200x60", Rect::new(0, 0, 200, 60)),
    ];
    let mut group = c.benchmark_group("blit_buffer");
    for (name, area) in sizes {
        let src = Buffer::empty(area);
        let mut dst = Buffer::empty(area);
        group.bench_function(name, |b| {
            b.iter(|| blit_buffer(black_box(&src), black_box(&mut dst), black_box(area)))
        });
    }
    // Also test clipped/partial blits
    group.bench_function("80x24 clipped offset", |b| {
        let src = Buffer::empty(Rect::new(0, 0, 160, 48));
        let mut dst = Buffer::empty(Rect::new(0, 0, 80, 24));
        let area = Rect::new(40, 12, 80, 24); // partially overlaps dst
        b.iter(|| blit_buffer(black_box(&src), black_box(&mut dst), black_box(area)))
    });
    group.finish();
}

criterion_group!(benches, bench_blit_buffer);
criterion_main!(benches);
