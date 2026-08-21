//! opencar — an open-world braille-graphics TUI driving simulator.
//!
//! Fully standalone: own crossterm terminal setup, own game loop, own
//! keybindings. Renders an infinite procedural world (hills, mountains,
//! winding highways, AI traffic) as Unicode braille dots so each frame looks
//! like grainy dashcam video. GPU backend behind `feature = "gpu"`; the CPU
//! renderer is always available.

pub mod app;
pub mod braille;
pub mod config;
pub mod display;
pub mod hud;
pub mod minimap;
pub mod render;
pub mod sim;
pub mod world;

pub use app::App;
