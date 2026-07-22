> _It *appears* to work if using zed to host the server (local zed, that is, connected to remote), but I don't think it's real.  Another thing to also try first is running the server directly on remote zed, and seeing if it works.

You are running into a classic macOS SSH limitation mixed with a major logical bug in your session client's codebase.

When you SSH into a remote Mac, `pbcopy` and `arboard` usually fail because the SSH session is completely detached from macOS's `WindowServer` and GUI Pasteboard context. The standard solution is exactly what you are attempting: falling back to **OSC 52**, which bypasses the remote macOS clipboard entirely and sends an escape sequence over the SSH byte stream directly to your local terminal emulator.

However, **OSC 52 will currently never work in your remote session client** because your extraction logic is looking in the wrong place.

### The Bug: The VT100 Black Hole

If you look at `crates/term-session-client/src/lib.rs`, here is how you are currently trying to extract the clipboard text:

```rust
// Detect screen changes
let current_content = {
    let parser = pane.shared_parser();
    let parser = parser.lock().unwrap();
    parser.screen().contents_formatted() // <-- THE RENDERED SCREEN BUFFER
};

// Process OSC 52 clipboard data
if has_new_data && let Some(text) = extract_osc52_text(&current_content) { // <-- THE BUG
    let _ = clipboard.set(&text);
}

```

You are passing `current_content` to the OSC 52 extractor. But `current_content` is the output of `contents_formatted()`, which represents the visual characters currently painted on the screen.

OSC (Operating System Command) codes are invisible control sequences. The `vt100::Parser` internally consumes and discards them because they aren't meant to be drawn. **The `contents_formatted()` buffer will never contain the `\x1b]52;` sequence because the parser already ate it.**

### The Fix: Extract from the Raw Byte Stream

To fix this, you must extract the OSC 52 payload from the **raw byte stream** exactly as it comes over the network from the server, *before* it gets fed into the VT100 parser.

Here are the exact changes to make in `crates/term-session-client/src/lib.rs`:

**1. Update your imports**
Change your clipboard import to bring in `Osc52Extractor` (which handles cross-chunk buffering over streams) instead of `extract_osc52_text`:

```rust
use term_wm_pty_engine::clipboard::{Clipboard, Osc52Extractor};

```

**2. Create a side-channel for clipboard text**
Right before the `rt.spawn` block that subscribes to the output, add a second channel to pass clipboard data back to the main thread:

```rust
    // Channel for raw PTY output bytes from the subscription stream
    let (push_tx, push_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    
    // NEW: Side-channel for intercepted OSC 52 clipboard text
    let (clip_tx, mut clip_rx) = mpsc::unbounded_channel::<String>();

```

**3. Intercept the raw bytes in the async task**
Update the `rt.spawn` block to feed the raw network chunks into the `Osc52Extractor` before passing them to the UI:

```rust
        rt.spawn(async move {
            let mut osc52 = Osc52Extractor::new();
            let mut prev_tail: [u8; 8] = [0; 8];

            while let Some(chunk) = reader.recv().await {
                match chunk {
                    Ok(data) => {
                        // Extract OSC 52 from the raw byte stream
                        if let Some(text) = osc52.push(&data, &prev_tail) {
                            let _ = clip_tx.send(text);
                        }

                        // Maintain history tail for cross-chunk sequences
                        let n = data.len();
                        if n >= 8 {
                            prev_tail.copy_from_slice(&data[n - 8..n]);
                        } else if n > 0 {
                            prev_tail.rotate_left(n);
                            prev_tail[8 - n..].copy_from_slice(&data[..n]);
                        }

                        let _ = push_tx.send(data);
                    }
                    Err(_) => break,
                }
            }
        });

```

**4. Apply the clipboard text in your main event loop**
Finally, delete the old flawed `extract_osc52_text(&current_content)` block in your main `loop`. Replace it with this to drain the side-channel:

```rust
        // Detect screen changes
        let current_content = {
            let parser = pane.shared_parser();
            let parser = parser.lock().unwrap();
            parser.screen().contents_formatted()
        };
        let has_new_data = prev_content.as_deref() != Some(&current_content);

        // NEW: Drain any clipboard texts intercepted by the reader task
        while let Ok(text) = clip_rx.try_recv() {
            let _ = clipboard.set(&text);
        }

```

Once you do this, your remote session client will cleanly intercept the OSC 52 sequence directly from the network stream and successfully write the copy payload to your local clipboard, entirely bypassing the macOS `pbcopy` SSH restrictions.
