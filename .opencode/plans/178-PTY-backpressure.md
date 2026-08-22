> _Can PTY backpressure be monitored as a metric in the HUD?_

<code_review_analysis>
The performance disparity between a Linux Virtual Terminal Interface (VTI) / Linux console and macOS `Terminal.app` is an architectural boundary issue regarding how terminal emulators process stream input:

1. **Rendering Subsystem Differences:**
* **Linux VTI / Console:** Writes character cells directly to the kernel TTY driver or a simple framebuffer (`fbcon`/DRM). Text layout, glyph shaping, font fallback, and window composition do not exist. Stream parsing is near-instantaneous.
* **macOS `Terminal.app`:** Uses legacy Quartz 2D / Core Graphics for CPU-side character layout and software rasterization. Every SGR escape sequence forces state updates in a single-threaded parser, and every glyph trigger routes through Core Text layout passes on the main thread.


2. **System Architectural Failure in Engine Design:**
* Treating `stdout` as an infinite sink for high-frequency $160 \times 90$ ANSI color streams (60–100 KB per frame at 60 Hz) ignores the consumer's throughput ceiling.
* When `Terminal.app`'s input buffer fills, `write()` blocks the process on the kernel PTY queue. Blaming the host terminal for requiring a GPU reveals a failure to enforce rate-limiting or stream-compression contracts at the application layer.
</code_review_analysis>



# Critical Flaws (SEV-1 & SEV-2)

### SEV-2: Unbounded Stream Generation for Low-Throughput Consumers

* **Location:** `display.rs`, `main.rs`
* **Flaw:** The engine emits unthrottled escape streams regardless of the downstream terminal's parsing throughput. In unaccelerated environments like `Terminal.app`, byte processing is order-of-magnitude slower than CPU frame generation, causing main-thread execution to stall inside `write()` or `flush()`.
* **Impact:** Frame rates collapse to single digits while host CPU utilization drops, masking rendering performance behind I/O blocking.

---

# Refactoring Directives

### 1. Enforce a Strict Per-Frame Byte Budget

* **Command:** Cap the total byte payload emitted per call to `present()`.
* **Implementation:**
* Track total bytes written to the `BufWriter` during a frame scan.
* If the byte count crosses a configurable threshold (e.g., $32\text{ KB}$ per frame), truncate further differential color updates for that frame and prioritize cursor positioning and critical HUD geometry.
* Defer suppressed cell updates to the next frame scan.



### 2. Implement Dynamic Color Mode Downscaling under PTY Backpressure

* **Command:** Automatically degrade color precision when PTY blocking exceeds the frame deadline.
* **Implementation:**
* Monitor the `blocked_ms` metric recorded during `present()`.
* If `blocked_ms` exceeds $30\%$ of `TARGET_FRAME_TIME` over 3 consecutive frames, automatically drop the emitter state from TrueColor ($\sim 22\text{ B/cell}$) or 256-color ($\sim 12\text{ B/cell}$) to basic 16-color ANSI ($\sim 5\text{ B/cell}$) or monochrome.
* Restore higher color depths only after 10 consecutive clean frames with zero PTY block time.



---

# Final Verdict

AMEND

---


<code_review_analysis>
The system behavior you are observing is precisely kernel-level PTY backpressure.

1. **The PTY Buffer Queue:**
* The operating system manages a Unix domain socket / pseudo-terminal (PTY) ring buffer between your application (`stdout` / slave PTY) and the terminal emulator process (`stdin` / master PTY). On macOS, this buffer is typically 64 KB.
* When your application writes faster than the terminal can consume, the PTY buffer fills. Once full, any synchronous `write()` or `flush()` call blocks the thread at the kernel level until the reading process (Terminal.app) drains bytes from the file descriptor.


2. **Terminal Consumption Bottlenecks:**
* **Linux VTI:** Direct kernel memory write with zero text parsing or font rasterization overhead. Drains PTY buffers nearly instantaneously.
* **Terminal.app:** Uses CPU-bound Quartz/CoreText text-rendering pipelines. Parsing complex, multi-byte SGR color escape sequences (`\x1b[38;2;...m`) forces heavy single-threaded text layout passes.
* **The Failure:** If your application generates 60–100 KB of ANSI escape sequences at 60 Hz, Terminal.app's processing loop saturates at ~10–15 frames per second. The resulting kernel-level block puts the `opencar` main thread to sleep, leading to the observed low CPU usage coupled with low frame rates.
</code_review_analysis>



# Critical Flaws (SEV-1 & SEV-2)

### SEV-2: Synchronous PTY Blocking Stalls Application Event Loop

* **Location:** `main.rs`, `display.rs`
* **Flaw:** Invoking blocking `flush()` calls directly on `stdout` allows a slow consumer (like Terminal.app) to dictate the execution frequency of the entire application. When the PTY buffer saturates, the kernel suspends the main thread mid-frame, causing input sampling and physics ticks to freeze along with rendering.
* **Impact:** Severe input latency, missed keyboard events, and frame pacing collapses when running in unaccelerated terminal emulators.

---

# Refactoring Directives

### 1. Decouple PTY Writes from the Physics / Input Loop

* **Command:** Never allow `stdout` I/O blocks to stall input polling or simulation ticks.
* **Implementation:**
* Maintain the fixed $60\text{ Hz}$ physics tick (`app.update()`) independent of output flushes.
* If a frame's `present()` call encounters a full buffer or exceeds its time budget during `flush()`, skip subsequent render steps until the output buffer clears while continuing to tick inputs and physics.



### 2. Implement Non-Blocking Frame Dropping on Backpressure

* **Command:** Treat rendering output as lossy state snapshots.
* **Implementation:**
* If PTY backpressure is detected (i.e., `b` / blocked time in `present()` exceeds the frame threshold), drop the current frame's output emission.
* Leave `self.prev` un-updated for skipped frames so that the next successful `present()` call automatically diffs against the last actually-emitted frame state without corrupting terminal tracking.



---

# Final Verdict

AMEND
