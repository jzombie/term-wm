# term-wm

**WORK IN PROGRESS.**

A cross-platform window manager for terminal shells.

Additional documentation available soon.

## Terminal Render Benchmark

The project now ships a standalone benchmark binary in [src/bin/render_bench.rs](src/bin/render_bench.rs). It produces an aggressive, animated noise field and reports frame pacing plus cell-update throughput so you can compare native terminal performance against `term-wm` hosting the same workload.

- **Standalone run:** `cargo run --release --bin render_bench -- --duration 15 --fps 120`
- **Inside term-wm:** `cargo run --release --bin term-wm -- "./target/release/render_bench --duration 15 --fps 120"`

The second form launches `term-wm` and feeds the benchmark command to the first pane. Run both variants back-to-back to see how much headroom the host terminal versus the managed window environment provides.

## License

`term-wm` is primarily distributed under the terms of both the MIT license and the Apache License (Version 2.0).

See [LICENSE-APACHE](./LICENSE-APACHE) and [LICENSE-MIT](./LICENSE-MIT) for details.
