//! Deadline-bounded asynchronous condition polling (tokio timers).

use std::future::Future;
use std::time::{Duration, Instant};

/// Interval between probe attempts in [`wait_for_async`].
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Poll an async `probe` until it returns `Some(value)`, or panic once
/// `deadline` elapses.
///
/// Async counterpart of [`crate::wait_for`]. The probe closure is invoked
/// repeatedly; each returned future is driven to completion, and the loop
/// sleeps on the tokio timer between attempts.
///
/// ```ignore
/// let conn = wait_for_async(Duration::from_secs(20), "gateway reachable", || {
///     async { RpcIpcClient::new(socket).await.ok() }
/// })
/// .await;
/// ```
///
/// # Panics
/// Panics if `probe` never returns `Some` within `deadline`.
pub async fn wait_for_async<T, F, Fut>(deadline: Duration, desc: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let start = Instant::now();
    loop {
        if let Some(value) = probe().await {
            return value;
        }
        assert!(
            start.elapsed() < deadline,
            "condition not met within {deadline:?}: {desc}"
        );
        tokio::time::sleep(DEFAULT_POLL_INTERVAL).await;
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn returns_probe_value_when_immediately_satisfied() {
        let value = wait_for_async(Duration::from_secs(1), "always true", || async {
            Some("ready")
        })
        .await;
        assert_eq!(value, "ready");
    }

    #[tokio::test]
    async fn polls_until_condition_becomes_true() {
        let ticks = Arc::new(AtomicU64::new(0));
        let seen = Arc::clone(&ticks);
        let value: u64 = wait_for_async(Duration::from_secs(2), "second tick", move || {
            let seen = Arc::clone(&seen);
            async move {
                let n = seen.fetch_add(1, Ordering::Release);
                (n >= 1).then_some(n)
            }
        })
        .await;
        assert_eq!(value, 1);
    }

    #[tokio::test]
    #[should_panic(expected = "condition not met within")]
    async fn panics_with_description_after_deadline() {
        let _: () = wait_for_async(
            Duration::from_millis(50),
            "impossible condition",
            || async { None },
        )
        .await;
    }
}
