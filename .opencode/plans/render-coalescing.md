When a background process (like a compiler or a heavy log stream) floods the terminal with rapid, erratic bursts of output, triggering a UI redraw for every single byte or data chunk would cause severe CPU thrashing, visual stuttering, and frame drops. To mitigate this bottleneck, modern terminal window managers delay rendering slightly to batch multiple updates into a single comprehensive frame.

Based on your notes, there are two primary scheduling mechanisms used to accomplish this:

**1. Delay Timers (The WezTerm Model)**
WezTerm implements a highly specific 3-millisecond coalescing delay (`mux_output_parser_coalesce_delay_ms`). When the background PTY thread signals the event loop that new data has arrived, the event loop does not redraw the screen immediately. Instead, it starts a 3ms timer. Any additional data that arrives within that brief window is seamlessly ingested into the text buffer, but it does not reset or extend the timer. Once the 3ms timer expires, a single unified visual frame is emitted to the user, eliminating screen flicker.

**2. Tick-Based "Pull" Architecture**
Instead of a "push" model where the background thread commands the UI to render, high-performance engines flip the scheduling to a "pull" architecture:
*   The main event loop runs on a steady, independent cycle (e.g., polling every 16.6ms to target 60 FPS).
*   Meanwhile, the background PTY thread continuously processes incoming bytes, mutates the underlying text grid, and toggles an `is_dirty` flag without waiting for a UI response.
*   When the main thread's 16.6ms tick arrives, it checks the `is_dirty` flag. If true, it pulls the data and renders only the *final* resolved state of the grid.

If the background thread processed fifty sequential text writes within that single 16.6ms window, this architecture naturally coalesces all of those changes and **completely drops the forty-nine intermediate visual states**. This guarantees that the terminal manager prioritizes input handling and layout stability over the doomed, CPU-draining task of trying to paint every microsecond of terminal output.
