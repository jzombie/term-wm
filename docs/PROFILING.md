## Profiling with `term-bench` (single-pane)

```bash
cd crates/term-bench && cargo build --release && cd ../..
```

```bash
samply record --save-only -o samply.json ./target/release/term-wm -n 1 "./target/release/term-bench"
```

---

## Profiling with `cat /dev/random` (dual-pane)

```bash
cargo build --release
```

```bash
samply record --save-only -o samply.json ./target/release/term-wm -n 2 "cat /dev/random" "cat /dev/random"
```
