/// Opaque render backend trait with downcasting capability.
/// Core crate defines this trait; UI crates downcast to concrete implementations.
/// This enables true backend independence — core compiles without Ratatui.
pub trait RenderBackend: std::any::Any {
    /// Downcast to concrete backend type.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;

    /// Returns a zero-initialized persistent mask slice sized to the current buffer.
    /// The mask is a flat Vec<u8> that decouples conditional string checks from
    /// bitwise buffer mutation — enabling SIMD-friendly two-pass rendering.
    /// Allocates only when the buffer grows; zero allocations in steady state.
    fn acquire_mask(&mut self) -> &mut [u8];
}

/// Abstraction over the terminal output backend.
/// Uses trait objects (dyn) at the compositor boundary for runtime flexibility.
/// Performance: vtable indirection is dispatched once per frame/window, not per cell.
pub trait RenderTarget {
    fn enter(&mut self) -> std::io::Result<()>;
    fn exit(&mut self) -> std::io::Result<()>;
    fn draw<F>(&mut self, f: F) -> std::io::Result<()>
    where
        F: FnOnce(&mut dyn RenderBackend);
    fn repair(&mut self) -> std::io::Result<()> {
        self.exit()?;
        self.enter()
    }
}
