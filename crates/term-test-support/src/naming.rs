//! Collision-free IPC channel names for tests.

use std::sync::atomic::{AtomicU64, Ordering};

/// Namespace segment shared by all test gateway channel names. Matches the
/// convention already used by the root integration harness and daemon tests.
const GATEWAY_TEST_NAMESPACE: &str = "term-wm";

/// Per-process monotonic counter distinguishing names within one test binary.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Build a unique gateway channel name: `term-wm/{tag}-{pid}-{n}`.
///
/// The pid component is what makes this safe beyond a single process: plain
/// per-process counters restart at 1 in every test binary, so two concurrent
/// runs (or a leftover daemon from a crashed prior run) could otherwise claim
/// the same name. Embedding the pid makes collisions require both same-name
/// reuse AND process-id reuse on the same machine within the leftover's
/// lifetime.
pub fn unique_gateway_name(tag: &str) -> String {
    let n = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{GATEWAY_TEST_NAMESPACE}/{tag}-{}-{n}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_embed_tag_pid_and_counter() {
        let name = unique_gateway_name("probe");
        let expected_prefix = format!("{GATEWAY_TEST_NAMESPACE}/probe-{}-", std::process::id());
        assert!(
            name.starts_with(&expected_prefix),
            "unexpected name shape: {name}"
        );
    }

    #[test]
    fn successive_names_differ() {
        let a = unique_gateway_name("dup");
        let b = unique_gateway_name("dup");
        assert_ne!(a, b);
    }
}
