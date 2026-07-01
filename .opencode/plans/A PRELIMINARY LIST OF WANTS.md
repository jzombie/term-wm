> _"It is about designing systems that are fundamentally safe by default, yet infinitely extensible by design."_

This is a high-level overview of what I'm looking for, but might not contain all of the intrinsic details on how to get there.  That's what the other context files adjacent to this list are for.

1. Dynamic configurability using Rust structs (IF eventually wanting to use a different format, it should be parsable into same Rust structs).  Application entrance points should be easily configurable, with the windows that they want to display the primary potentially imperative logic, as part of a builder pattern.  Everything else should probably be part of the configuration. The initialization sequence should be identicial regardless if the app is "standalone" or "embedded".  The *core* distinction between standalone or embedded modes should simply be a configuration difference.
2. Better application lifecycle management.
3. Better threading model (see thread-model.md)
4. "Dirty rectangles" damage control should be a separate workspace crate that can work outside of the window manager.  I have a prototype server/client scenario where the client is a dummy buffer render, and the server/client should also take advantage of this algorithm. This change calculation should also be usable for calculating deltas if the rendering is going to be performed over network. For usage with Ratatui, it *must use* Direct Buffer Access.  If this requires an API change in term-wm, that's *okay*, it just needs to be done. [See ratatui-direct-buffer-access.md]
5. Render coalescing [see render-coalescing.md]
6. Configurable scrollback buffering that can scale to millions without impacting performance or balooning memory (goal, a few hundred megabytes of RAM with several million lines of scrollback).  Research advanced memory paging.
7. Snapshot testing using `insta` crate
