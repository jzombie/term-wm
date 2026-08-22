//! Background chunk streaming (Stage 0f).
//!
//! One worker owns copies of the immutable generators (`Noise` and
//! `RoadNetwork` are both `Copy`) and bakes requested chunks in parallel on
//! the shared bounded rayon pool. The main thread never blocks:
//!
//! 1. `collect()` drains finished bakes and dropped-key tombstones without
//!    waiting, inserting fresh chunks into the world;
//! 2. `request()` admits at most `MAX_INFLIGHT_CHUNKS − in_flight.len()`
//!    candidates, so batch sizes can never exceed the result channel's
//!    capacity.
//!
//! Contract details (see plan §0f): results that arrive with a full channel
//! emit a tombstone that moves the key into a short retry backoff instead of
//! thrashing; an epoch counter invalidates stale batches per-chunk before
//! baking; `collect` always runs before `request` within a frame.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::config::*;
use crate::world::World;
use crate::world::chunk::Chunk;
use crate::world::noise::Noise;
use crate::world::roads::RoadNetwork;

/// Work order: one batch of chunk keys plus the generation it was requested
/// under.
struct BakeBatch {
    epoch: u64,
    keys: Vec<(i32, i32)>,
}

/// Worker loop: owns generator copies, bakes batches in parallel on the
/// shared pool, ships results back. Exits when every work sender drops.
fn worker(
    noise: Noise,
    roads: RoadNetwork,
    epoch: Arc<AtomicU64>,
    work_rx: mpsc::Receiver<BakeBatch>,
    result_tx: mpsc::SyncSender<((i32, i32), Chunk)>,
    drop_tx: mpsc::Sender<(i32, i32)>,
) {
    use rayon::prelude::*;

    // Bake one key with the per-chunk staleness guard applied.
    let bake_one = |batch_epoch: u64, key: (i32, i32)| -> Option<((i32, i32), Chunk)> {
        if epoch.load(Ordering::Relaxed) != batch_epoch {
            return None;
        }
        Some((key, Chunk::bake(key.0, key.1, &noise, &roads)))
    };

    for (batch_epoch, keys) in work_rx.iter().map(|b| (b.epoch, b.keys)) {
        // Micro-batches stay on this worker thread: rayon dispatch overhead
        // exceeds the ~0.4 ms scalar bake. Larger batches fan out across
        // the SHARED bounded pool (never rayon's global default pool, which
        // oversubscribes march during render).
        let results: Vec<_> = if keys.len() <= STREAM_SEQUENTIAL_BATCH_MAX {
            keys.into_iter()
                .filter_map(|k| bake_one(batch_epoch, k))
                .collect()
        } else {
            crate::pool::pool().install(|| {
                keys.into_par_iter()
                    .filter_map(|k| bake_one(batch_epoch, k))
                    .collect()
            })
        };

        for res in results {
            if let Err(mpsc::TrySendError::Full((key, _))) = result_tx.try_send(res) {
                let _ = drop_tx.send(key);
            }
        }
    }
}

/// Owns the background baking worker and its request bookkeeping.
pub struct ChunkStreamer {
    epoch: Arc<AtomicU64>,
    work_tx: Option<mpsc::Sender<BakeBatch>>,
    result_rx: mpsc::Receiver<((i32, i32), Chunk)>,
    drop_rx: mpsc::Receiver<(i32, i32)>,
    /// Keys whose batches were sent but not yet collected, with send time
    /// (capacity-capped by MAX_INFLIGHT_CHUNKS).
    in_flight: HashMap<(i32, i32), Instant>,
    /// Dropped-on-full keys cooling down before they may be re-requested.
    backoff: HashMap<(i32, i32), Instant>,
    player_chunk: (i32, i32),
}

impl ChunkStreamer {
    /// Spawn the baking worker. Generator values are copied into the thread;
    /// nothing is shared with [`World`] until insertion.
    pub fn spawn(noise: Noise, roads: RoadNetwork) -> Self {
        let epoch = Arc::new(AtomicU64::new(0));
        let (work_tx, work_rx) = mpsc::channel::<BakeBatch>();
        let (result_tx, result_rx) = mpsc::sync_channel::<((i32, i32), Chunk)>(MAX_INFLIGHT_CHUNKS);
        let (drop_tx, drop_rx) = mpsc::channel::<(i32, i32)>();

        let worker_epoch = Arc::clone(&epoch);
        let spawned = std::thread::Builder::new()
            .name("opencar-chunk-baker".into())
            .spawn(move || worker(noise, roads, worker_epoch, work_rx, result_tx, drop_tx));

        // If the OS refused the thread, keep the streamer as a harmless
        // no-op; synchronous loading paths still function.
        let work_tx = if spawned.is_ok() { Some(work_tx) } else { None };

        Self {
            epoch,
            work_tx,
            result_rx,
            drop_rx,
            in_flight: HashMap::new(),
            backoff: HashMap::new(),
            player_chunk: (0, 0),
        }
    }

    /// True while at least one admission slot is free.
    pub fn has_capacity(&self) -> bool {
        self.in_flight.len() < MAX_INFLIGHT_CHUNKS
    }

    /// Call once per frame with the player's current chunk coordinates.
    /// Crossing a chunk boundary bumps the generation, so any batches still
    /// queued behind the camera skip their bakes entirely.
    pub fn note_player_chunk(&mut self, ckx: i32, cky: i32) {
        if self.player_chunk != (ckx, cky) {
            self.player_chunk = (ckx, cky);
            // Every outstanding batch is now stale by definition — the
            // worker filters all its keys, so their result messages never
            // arrive. Purge the ghost slots immediately instead of starving
            // admission until the TTL sweep.
            self.in_flight.clear();
            self.epoch.fetch_add(1, Ordering::Release);
        }
    }

    /// Non-blocking drain of finished bakes and dropped-key tombstones.
    ///
    /// Tombstones move their key from `in_flight` into `backoff` rather than
    /// clearing it, so saturated channels can never starve brand-new
    /// view-front requests (review ×2). Must run before
    /// [`Self::request`](Self::request) each frame.
    pub fn collect(&mut self, world: &mut World, now: Instant) {
        while let Ok(((kx, ky), chunk)) = self.result_rx.try_recv() {
            self.in_flight.remove(&(kx, ky));
            world.insert_chunk(kx, ky, chunk);
        }
        while let Ok(key) = self.drop_rx.try_recv() {
            self.in_flight.remove(&key);
            self.backoff
                .insert(key, now + Duration::from_millis(REQUEST_BACKOFF_MILLIS));
        }
    }

    /// Admit up to `MAX_INFLIGHT_CHUNKS − in_flight.len()` missing chunks,
    /// filtering out anything in flight or still cooling down.
    pub fn request(&mut self, wants: &[(i32, i32)], now: Instant) {
        // Last-resort sweeps: a lost result (every accepted key produces
        // exactly one message, so this shouldn't happen) or an expired
        // cooldown must free admission capacity again.
        let ttl = Duration::from_millis(PENDING_TTL_MILLIS);
        self.backoff.retain(|_, expires| now < *expires);
        self.in_flight
            .retain(|_, queued| now.duration_since(*queued) < ttl);

        let Some(work_tx) = &self.work_tx else { return };
        let room = MAX_INFLIGHT_CHUNKS.saturating_sub(self.in_flight.len());
        if room == 0 {
            return;
        }
        let keys: Vec<(i32, i32)> = wants
            .iter()
            .filter(|key| !self.in_flight.contains_key(*key) && !self.backoff.contains_key(*key))
            .copied()
            .take(room)
            .collect();
        if keys.is_empty() {
            return;
        }
        for key in &keys {
            self.in_flight.insert(*key, now);
        }
        let _ = work_tx.send(BakeBatch {
            epoch: self.epoch.load(Ordering::Relaxed),
            keys,
        });
    }

    /// Test/inspection accessors.
    #[cfg(test)]
    pub fn test_counts(&self) -> (usize, usize) {
        (self.in_flight.len(), self.backoff.len())
    }

    #[cfg(test)]
    pub fn test_has_backoff(&self, key: &(i32, i32)) -> bool {
        self.backoff.contains_key(key)
    }

    /// Simulate a worker-side drop (channel full): moves the key out of
    /// in-flight into the cooldown set. Deterministic stand-in for the real
    /// tombstone round-trip.
    #[cfg(test)]
    pub fn test_note_dropped(&mut self, key: (i32, i32), now: Instant) {
        self.in_flight.remove(&key);
        self.backoff
            .insert(key, now + Duration::from_millis(REQUEST_BACKOFF_MILLIS));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAR_X: f32 = 100_000.0;
    const FAR_Z: f32 = 100_000.0;

    fn far_want(world: &World) -> Vec<(i32, i32)> {
        world.missing_chunks_around(FAR_X, FAR_Z)
    }

    fn wait_until(deadline_ms: u64, mut tick: impl FnMut() -> bool) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed().as_millis() < u128::from(deadline_ms) {
            if tick() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    #[test]
    fn request_collect_roundtrip_inserts_chunks() {
        let mut world = World::new(3);
        let mut streamer = ChunkStreamer::spawn(Noise::new(3), RoadNetwork::new());
        let wants = far_want(&world);
        assert!(wants.len() >= 2);

        let now = Instant::now();
        streamer.collect(&mut world, now); // no-op drain first
        streamer.request(&wants[..2], now);

        let both_loaded = |world: &World, wants: &[(i32, i32)]| {
            wants[..2]
                .iter()
                .all(|(kx, ky)| world.chunk_loaded(*kx, *ky))
        };
        let ok = wait_until(2_000, || {
            let now = Instant::now();
            streamer.collect(&mut world, now);
            both_loaded(&world, &wants)
        });
        assert!(ok, "background bake did not land within timeout");
        let (infl, bko) = streamer.test_counts();
        assert_eq!((infl, bko), (0, 0), "bookkeeping must drain to zero");
    }

    #[test]
    fn dropped_key_enters_backoff_and_is_not_re_admitted_early() {
        let world = World::new(5);
        let mut streamer = ChunkStreamer::spawn(Noise::new(5), RoadNetwork::new());
        let wants = far_want(&world);
        let key = wants[0];

        let t0 = Instant::now();
        streamer.request(std::slice::from_ref(&key), t0);
        assert!(streamer.test_counts().0 == 1, "admitted");

        // Simulate the worker dropping it on a full result channel.
        streamer.test_note_dropped(key, t0);
        assert!(streamer.test_has_backoff(&key));

        // While cooling down, a fresh request MUST NOT re-admit it.
        let t1 = t0 + Duration::from_millis(50);
        streamer.request(&[key], t1);
        assert_eq!(
            streamer.test_counts(),
            (0, 1),
            "backoff entry must block early re-admission"
        );

        // After the cooldown expires the sweep clears it and admission works.
        let t2 = t0 + Duration::from_millis(REQUEST_BACKOFF_MILLIS + 50);
        streamer.request(&[key], t2);
        assert!(streamer.test_counts().0 == 1, "re-admitted after backoff");
    }

    #[test]
    fn stale_in_flight_entries_are_swept_by_ttl() {
        let world = World::new(7);
        let mut streamer = ChunkStreamer::spawn(Noise::new(7), RoadNetwork::new());
        let key = far_want(&world)[0];

        // Admit, then pretend the result was lost long ago by aging the
        // queued timestamp directly through a fresh admission at an old
        // wall-clock point.
        let ancient = Instant::now() - Duration::from_millis(PENDING_TTL_MILLIS + 100);
        streamer.request(std::slice::from_ref(&key), ancient);
        // The ancient timestamp makes the TTL sweep retire the entry, so a
        // second request re-admits instead of silently starving forever.
        let now = Instant::now();
        streamer.request(&[key], now);
        assert_eq!(streamer.test_counts().0, 1, "re-admitted after TTL");
    }
}
