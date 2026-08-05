# term-size-box

[![made-with-rust][rust-logo]][rust-src-page]

Draws a dotted-line border overlay showing terminal dimensions that updates on resize.

Useful for verifying that a terminal emulator or window manager correctly forwards
terminal resize events and that the application sees the updated size.

Used in [term-wm](https://crates.io/crates/term-wm) as a test tool for verifying
resize propagation through nested terminal sessions.

## Build

From the workspace root:

```bash
cargo build -p term-size-box --release
```

## Usage

```bash
cargo run -p term-size-box
```

The overlay fills the screen with a dark background, draws a measured border, and
shows the current terminal dimensions centered inside it. Resize the terminal to
see the dimensions update live.

Press `q`, `Esc`, or `Ctrl-C` to exit.

## License

`term-size-box` is primarily distributed under the terms of both the MIT
license and the Apache License (Version 2.0).

See [LICENSE-APACHE](../../LICENSE-APACHE) and [LICENSE-MIT](../../LICENSE-MIT) for details.

[rust-src-page]: https://www.rust-lang.org/
[rust-logo]: https://img.shields.io/badge/Made%20with-Rust-black?logo=Rust
