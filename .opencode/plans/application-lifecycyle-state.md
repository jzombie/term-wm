Here is a breakdown of how your notes define and manage the lifecycle of applications and windows:

**Canonical Window Lifecycle States**
The research defines a strict state machine for window lifecycles based on standard GUI display server protocols (like X11 and Wayland):
*   **Realized:** The window is allocated memory and IDs, but no rendering intent is declared (invisible).
*   **Mapped:** The application requests to be drawn; the window manager computes its geometry and routes it to the layout tree (visible).
*   **Unmapped / Withdrawn:** The window is programmatically hidden and retains no spatial footprint, but remains in memory.
*   **Iconic / Minimized:** The window is mapped but hidden from the primary workspace layout.
*   **Shaded:** A localized unmapping where the client area is hidden, but the window's chrome (like the title bar) remains mapped and interactive.

**The Single Source of Truth**
A major architectural focus of the notes is preventing "zombie" windows and memory leaks that occur when a window's lifecycle becomes desynchronized across different parts of the application. 
To solve this, the architecture dictates that **all authoritative window data (the PTY file descriptor, text buffer, and child PID) must live in a central Generational Arena (Slotmap)**. The layout trees and rendering z-orders should never hold actual window data; they only hold lightweight `WindowKey` references. When a window is closed and removed from the central Slotmap, all downstream keys instantly become invalid, making out-of-sync lifecycles and "use-after-free" bugs mathematically impossible.

**The Boundary Between UI and Application Runner**
The notes emphasize establishing a strict, event-driven boundary between the visual window and the underlying application process to prevent issues like leaving a headless subprocess running after a window closes:
*   **The Window Server (UI):** Acts as the ultimate arbiter of *visual existence*. It processes user commands to close windows or handle layout constraints.
*   **The Application Runner (PTY):** Acts as a background I/O task mapped to a specific `WindowKey`.

**Handling Exits and Teardown**
The lifecycle concludes through two primary pathways, both of which must be handled **asynchronously** to prevent the UI from deadlocking while waiting for a process to close:
*   **User-Initiated Closure:** When a user explicitly closes a pane, the Window Server immediately deletes the window from the Slotmap. The system then explicitly assumes the responsibility of sending a `SIGHUP` or `SIGKILL` to the child application's PID and closing the pseudoterminal file descriptors, ensuring the process dies at the exact moment the visual representation is removed.
*   **Natural Application Exit:** If the child application exits naturally (e.g., the user types `exit` in a bash session), the background PTY thread detects the End-of-File (EOF) on the file descriptor. Crucially, this background thread does not directly mutate the UI. Instead, it sends an `AppExited(WindowKey)` control message to the main event loop, which then safely removes the window from the UI, establishing a clean lifecycle flow devoid of race conditions.
