[] Include examples how to share data between components.

---

**term-wm** is a blazingly fast, cross-platform terminal multiplexer, window manager, and Ratatui component library written entirely in Rust. It is designed to bring a "Retro-Modern" aesthetic to your workflow by seamlessly translating advanced graphical desktop metaphors—such as **floating windows, dynamic tiling, drag-and-drop resizing, and overlapping layers**—directly into the standard terminal character grid. 

Whether you are a power user looking for a better terminal environment or a Rust developer building your own tools, it is built to serve two distinct purposes:

**For Users: A Frictionless Standalone Workspace**
As a standalone application, term-wm gives you a complete window-managed environment that works flawlessly across macOS, Linux, Windows, and even over SSH using both your keyboard and mouse. Its biggest selling point is the **"No-Conflict" keybinding philosophy**. Instead of forcing you to use awkward prefix chords like `Ctrl+b`, it uses the `Esc` key as an intelligent, context-aware modifier. This guarantees that the window manager will **never fight for control over the keyboard shortcuts of your nested applications** like Neovim, `tmux`, or `screen`. If you need to pass an Escape key to the underlying app, you simply double-tap it.

**For Developers: A Plug-and-Play Layout Engine**
If you build TUI apps in Rust, term-wm can be embedded directly into your projects as a library. Instead of manually coding complex layout math, **term-wm provides a pre-built layout engine that automatically handles standardized focus cycles, precise Z-ordering for overlapping floating components, and complete window lifecycles**. You can initialize it in a full `standalone()` mode complete with top panels and system chrome, or an `embedded()` mode that integrates minimally into your existing Ratatui canvas.

**Uncompromised, Hardware-Driven Performance**
Under the hood, **term-wm is engineered to guarantee a buttery-smooth 60 to 120 FPS rendering pipeline without draining your laptop battery**. As we discussed previously, it leverages extreme micro-architectural optimizations:
*   **Data-Oriented Event Routing:** It uses the Hitbox Pattern and Generational Arenas (SlotMaps) to achieve zero-math, instantaneous mouse-click routing.
*   **Direct Memory Access (DMA):** It completely bypasses standard immediate-mode bottlenecks using direct buffer blitting.
*   **Render Coalescing & Deficit Round-Robin:** It dynamically throttles CPU usage and manages multiplexed network data so that massive background compiler logs never stall your typing latency.

In short, **term-wm** gives you the absolute power, visual depth, and layout flexibility of a graphical desktop window manager, perfectly engineered for the uncompromising speed and constraints of the terminal.
