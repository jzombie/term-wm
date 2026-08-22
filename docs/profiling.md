## Profiling with Debug Symbols (Verbose)

```bash
CARGO_PROFILE_RELEASE_DEBUG=true CARGO_PROFILE_RELEASE_SPLIT_DEBUGINFO=packed cargo build --release -v
```

## Profiling without the daemon

```bash
samply record --save-only -o samply.json ./target/debug/term-wm --no-session-persistence
```

## Profiling with `term-bench` (single-pane)

```bash
cd crates/term-bench && cargo build --release && cd ../..
```

> _Note: This also skips running the daemon._

```bash
samply record --save-only -o samply.json ./target/release/term-wm --no-session-persistence -n 1 "./target/release/term-bench"
```

---

## Profiling with `cat /dev/random` (dual-pane)

```bash
cargo build --release
```

> _Note: This also skips running the daemon._

```bash
samply record --save-only -o samply.json ./target/release/term-wm --no-session-persistence -n 2 "cat /dev/random" "cat /dev/random"
```




---

## Cross-Platform Idle Wakeup & Context Switch Metrics

macOS Activity Monitor is the only mainstream task manager that exposes an **Idle Wake Ups** column by default. On Linux and Windows you have to look at **CPU Wakeups**, **Context Switches**, or **Timer Expirations** using dedicated profiling tools.

### 1. macOS

Activity Monitor's **Idle Wake Ups** column is the most direct GUI metric available on any platform. For deeper analysis, use the command-line tool `powermetrics`.

#### Key Tools & Metrics

* **Activity Monitor (GUI):**
Add the **Idle Wake Ups** column via View → Columns. This shows per-process wakeups/sec — how many times a second a thread rouses the CPU from an idle state.

* **`powermetrics` (CLI):**
Provides per-process wakeup and interrupt statistics at a configurable sampling interval.
```bash
sudo powermetrics --samplers tasks -i 1000
```

### 2. Linux

Linux offers power-management tools with a direct equivalent to macOS's idle wakeups metric.

#### Key Tools & Metrics

* **`powertop`:**
The closest cross-platform equivalent to macOS's idle wakeup tracking. Shows a **`Wakeups/sec`** column per process and timer, measuring how many times per second a thread forces a CPU core out of an idle C-state.
```bash
sudo powertop
```

* **`pidstat` (Per-Process Context Switches):**
Shows how many times per second a specific process gives up or reclaims CPU execution time.
```bash
pidstat -w 1
# Looks for: cswch/s (voluntary) and nvcswch/s (involuntary) context switches
```

* **`vmstat` (System-Wide Interrupts):**
Provides a quick, real-time snapshot of system-wide interrupts (`in`) and context switches (`cs`) per second.
```bash
vmstat 1
```

### 3. Windows

Windows Task Manager does not show CPU wakeups. Instead, Windows measures **Context Switches per Second** and **Timer Resolution requests**.

#### Key Tools & Metrics

* **Performance Monitor (`perfmon`):**
Windows' built-in system profiler allows you to track context switches per thread or process.
1. Press `Win + R`, type `perfmon`, and hit Enter.
2. Add the counter: **Thread → Context Switches/sec** or **Process → Thread Count / Context Switches**.
3. This tracks how many times per second Windows switches execution to that thread.

* **Windows Performance Analyzer (WPA / WPT):**
Microsoft's official deep-dive profiling suite (part of the Windows Assessment and Deployment Kit). The **CPU Usage (Precise)** graph logs exact thread transitions, timer expirations, and CPU C-state wakeups per process down to the microsecond.

* **`powercfg` (Timer Resolution Analysis):**
A common cause of high CPU wakeups on Windows is apps requesting high-frequency system timers (e.g., requesting a 1ms timer tick instead of the default 15.6ms).
```cmd
powercfg /energy
```
This generates an HTML report listing any processes keeping the Windows timer resolution unnaturally high.

### Metric Mapping Summary

| Operating System | Default GUI | Specialized Tool | Exact Metric to Look For |
| --- | --- | --- | --- |
| **macOS** | Activity Monitor | `powermetrics` | **Idle Wake Ups** (wakeups/sec) |
| **Linux** | N/A | `powertop` / `pidstat` | **`Wakeups/sec`** / **`cswch/s`** |
| **Windows** | N/A | Performance Monitor / WPA | **`Context Switches/sec`** |
