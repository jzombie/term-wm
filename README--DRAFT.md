> "Terminal Desktop Environment (TDE)" or a "Spatial Terminal Compositor

Fine-Tuned: "The Terminal Desktop Environment (TDE)" — Subtitle: "The Graphical Desktop for SSH."
The "Why": The suffix "-wm" (window manager) is deeply linked to X11/Wayland tiling managers like i3 or bspwm, which carry a reputation for barren default states and hours of configuration script fatigue. Explicitly defining term-wm as a Terminal Desktop Environment (TDE) signals that the application delivers complete operational chrome—panels, tasks, launchers, overlays, and floating windows—out of the box, natively inside the standard terminal grid.

> "Drag a window over an SSH connection"

Fine-Tuned: "True Spatial Window Compositing Over Secure Shell (SSH)."
The "Why": Local tiling window managers and graphical multiplexers require local GPU-accelerated display environments and fail completely over SSH. Legacy terminal multiplexers like tmux limit users to rigid, mathematically planar grids. Your hybrid layout compositor shatters this divide by rendering smooth mouse dragging, dynamic edge-snapping with ghost outlines, and z-ordered drop shadows with depth shading directly over standard, headless SSH connections.

> Native terminal session persistence on Windows, macOS, and Linux.

Fine-Tuned: "Zero-Setup Session Persistence via an Embedded PTY Daemon."
The "Why": Unlike traditional tools that require manual daemon commands, socket configurations, or third-party persistence plugins, term-wm bundles a lightweight (~9 MB) compositor and background session gateway into a single unprivileged binary. Workspaces, window dimensions, and running pseudo-terminal (PTY) child processes persist automatically across network drops, local client exits, and system restarts.

> Share sessions with multiple users, with attributed IPC event pipeline (events carry who did what)

The "Why": Sharing terminal sessions historically forced users into a single uniform viewport that shrank to the smallest screen and relied on highly insecure socket permissions (like chmod 777 /tmp/socket). By routing inputs through an attributed muxio RPC pipeline, term-wm tags every keyboard and mouse action with a unique viewer connection ID. This enables independent viewport sizing, isolated mouse capture, and allows the host to selectively evict an individual participant without terminating the underlying processes or disconnecting other collaborators.

> Floating, tiling, and monocle (for mobile) window modes

Fine-Tuned: "Unified Window Topology: BSP Tiling, Free-Floating Stacks, and Mobile Monocle Modes."
The "Why": This communicates that your tiling engine isn't restricted to a flat, planar split. It mathematically unifies tree-based, integer-ratio Binary Space Partitioning (BSP) with a z-ordered, overlapping floating window layer and responsive full-screen views, adapting to any workflow geometry.

> Responsive visual layouts which automatically trigger monocle or preferred window mode, with the ability to override automatic settings

Fine-Tuned: "Zero-Loss Layout Reflowing for On-the-Go Mobile Resumption."
The "Why": Splitting a restricted mobile viewport (like an iPad running Blink Shell or a phone using Termux) into multiple panes compresses text into illegible, narrow columns. The engine preserves your complex desktop layout tree in memory while automatically collapsing the visual output into a full-screen Monocle Mode. To preserve mobile battery and CPU, background panes execute asynchronously in memory while occluded widgets skip rendering. When reattached to a larger monitor, the engine automatically restores your original tiled layouts exactly as they were.

> Project-specific tasks.json for Quick Actions launching directly from the Command Palette

Fine-Tuned: "Context-Aware Task Integration and Non-Blocking Process Isolation."
The "Why": Rather than forcing developers to write manual shell scripts or switch panes to run builds and test suites, term-wm automatically discovers project-specific .term-wm/tasks.json configs in your current directory. These tasks register as instant, searchable items inside the nucleo-powered Command Palette, executing in background-isolated PTYs. When a process exits, term-wm keeps the pane open and appends a distinct process exit marker, ensuring compilation errors or test failures are never lost or cleared.

> It strives to understand the context of the tools you use and configures itself accordingly

Fine-Tuned: "Zero-Friction Context-Awareness and Autonomous Input Routing."
The "Why": Traditional multiplexers force developers to constantly fiddle with manual settings, configuration overrides, or rigid prefix chords (like Ctrl+B) that collide with native commands inside Neovim, Emacs, or shell environments. term-wm completely eliminates this operational friction. By passively monitoring terminal streams via an embedded, low-overhead state machine (PtyStateTracker), it autonomously identifies active alternate-buffer or mouse-tracking requests (such as CSI mode toggles). The moment it detects an interactive application taking over, the window manager instantly steps out of the way into a zero-delay, unbuffered Direct Input Mode, yielding complete control to the application without any manual keystrokes or mode switching. The desktop environment dynamically bends to match the vocabulary of your focused tools—leaving your critical workflows completely undisturbed.

> [partially true; needs addressing] "I despise prefix chords so much that I built a custom ANSI parser to track state changes in real-time"

> Shatter the assumption that terminals are rigid grids (animated GIF demo)
