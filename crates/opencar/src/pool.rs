//! Shared bounded rayon pool.
//!
//! Both parallel consumers (terrain-march bands, background chunk bakes) run
//! on one pool sized to `available_parallelism - 1` so a core always stays
//! free for the main event/render thread — no starvation on small machines.

use std::sync::OnceLock;

/// Process-wide pool, built once on first use.
pub fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        rayon::ThreadPoolBuilder::new()
            .num_threads(cores.saturating_sub(1).max(1))
            .build()
            .expect("rayon pool build")
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn pool_leaves_one_core_free() {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        let expected = cores.saturating_sub(1).max(1);
        assert_eq!(super::pool().current_num_threads(), expected);
    }
}
