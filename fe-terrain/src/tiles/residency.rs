//! Tile-byte residency: a pinned, byte-bounded LRU and the async `TileResidency` loader; see `src/AGENTS.md` §tile-residency.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

/// Byte budget for one hexon source's tile cache (evidence §5; constant, no knob).
pub const TILE_BYTE_BUDGET: u64 = 256 * 1024 * 1024;

/// Max tile loads in flight at once (bounds the IoTaskPool backlog).
pub const MAX_TILE_LOADS_IN_FLIGHT: usize = 16;

/// Frames a loaded-but-unspawned tile is held before it is dropped.
pub const READY_TTL_FRAMES: u64 = 300;

/// One cache slot: the value, its charged bytes, live pins and LRU tick.
struct LruSlot<V> {
    value: V,
    bytes: u64,
    pins: u32,
    tick: u64,
}

/// Byte-bounded LRU whose pinned entries are never evicted (LRU pattern + pin counts).
///
/// Only unpinned entries sit in `order`, so eviction is "pop the oldest unpinned".
pub struct ByteLru<K, V> {
    budget: u64,
    total: u64,
    tick: u64,
    slots: HashMap<K, LruSlot<V>>,
    order: BTreeMap<u64, K>,
}

impl<K: Eq + Hash + Clone, V: Clone> ByteLru<K, V> {
    /// Empty cache holding at most `budget` unpinned+pinned bytes (pinned may overshoot).
    pub fn new(budget: u64) -> Self {
        Self {
            budget,
            total: 0,
            tick: 0,
            slots: HashMap::new(),
            order: BTreeMap::new(),
        }
    }

    fn next_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    /// Cached value for `key`, refreshing its recency.
    pub fn get(&mut self, key: &K) -> Option<V> {
        let tick = self.next_tick();
        let slot = self.slots.get_mut(key)?;
        if slot.pins == 0 {
            self.order.remove(&slot.tick);
            self.order.insert(tick, key.clone());
        }
        slot.tick = tick;
        Some(slot.value.clone())
    }

    /// Whether `key` is cached.
    pub fn contains(&self, key: &K) -> bool {
        self.slots.contains_key(key)
    }

    /// Insert (or replace, keeping pins) `key`, then evict LRU unpinned entries over budget.
    pub fn insert(&mut self, key: K, value: V, bytes: u64) {
        let tick = self.next_tick();
        let pins = match self.slots.remove(&key) {
            Some(old) => {
                self.total -= old.bytes;
                if old.pins == 0 {
                    self.order.remove(&old.tick);
                }
                old.pins
            }
            None => 0,
        };
        if pins == 0 {
            self.order.insert(tick, key.clone());
        }
        self.total += bytes;
        self.slots.insert(
            key,
            LruSlot {
                value,
                bytes,
                pins,
                tick,
            },
        );
        self.evict_to_budget();
    }

    /// Pin `key` (insert it first when absent) so eviction skips it until released.
    pub fn insert_pinned(&mut self, key: K, value: V, bytes: u64) {
        if !self.slots.contains_key(&key) {
            self.insert(key.clone(), value, bytes);
        }
        self.pin(&key);
    }

    /// Add one pin to a cached `key`; false when it is not cached.
    pub fn pin(&mut self, key: &K) -> bool {
        let Some(slot) = self.slots.get_mut(key) else {
            return false;
        };
        if slot.pins == 0 {
            self.order.remove(&slot.tick);
        }
        slot.pins += 1;
        true
    }

    /// Drop one pin; the entry leaves the cache once no pin holds it (exit-ring eviction).
    pub fn release(&mut self, key: &K) {
        let Some(slot) = self.slots.get_mut(key) else {
            return;
        };
        if slot.pins > 1 {
            slot.pins -= 1;
            return;
        }
        if let Some(slot) = self.slots.remove(key) {
            self.total -= slot.bytes;
            if slot.pins == 0 {
                self.order.remove(&slot.tick);
            }
        }
    }

    /// Drop every pinned entry (the active composite was replaced).
    pub fn release_all_pins(&mut self) {
        let total = &mut self.total;
        self.slots.retain(|_, slot| {
            if slot.pins > 0 {
                *total -= slot.bytes;
                false
            } else {
                true
            }
        });
    }

    /// Drop every unpinned entry (the backing archive was replaced on disk).
    pub fn clear_unpinned(&mut self) {
        let total = &mut self.total;
        self.slots.retain(|_, slot| {
            if slot.pins == 0 {
                *total -= slot.bytes;
                false
            } else {
                true
            }
        });
        self.order.clear();
    }

    fn evict_to_budget(&mut self) {
        while self.total > self.budget {
            let Some((_, key)) = self.order.pop_first() else {
                return;
            };
            if let Some(slot) = self.slots.remove(&key) {
                self.total -= slot.bytes;
            }
        }
    }

    /// Bytes currently held (pinned + unpinned).
    pub fn total_bytes(&self) -> u64 {
        self.total
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// True when nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Number of pinned entries.
    pub fn pinned_len(&self) -> usize {
        self.slots.values().filter(|slot| slot.pins > 0).count()
    }
}

#[cfg(feature = "render")]
pub use render_residency::*;

#[cfg(feature = "render")]
mod render_residency {
    use std::collections::HashMap;

    use bevy::prelude::Resource;
    use bevy::tasks::{futures::check_ready, IoTaskPool, Task, TaskPool};

    use super::{MAX_TILE_LOADS_IN_FLIGHT, READY_TTL_FRAMES};
    use crate::tiles::composite::{CompositeTileSource, LoadedTile};
    use crate::tiles::TileCoord;

    type TileKey = (u8, u32, u32);

    fn key_of(coord: TileCoord) -> TileKey {
        (coord.zoom, coord.x, coord.y)
    }

    /// Outcome of [`TileResidency::ensure_resident`].
    pub enum Residency {
        /// Bytes are in hand; spawn now.
        Ready(LoadedTile),
        /// A load is in flight (or queued behind the in-flight bound); skip, never fail.
        Pending,
        /// No covering index (or cache) holds the tile: a real failure.
        Absent,
    }

    /// Async tile residency for the active petal: off-thread loads, pending ≠ failed.
    #[derive(Resource, Default)]
    pub struct TileResidency {
        revision: u64,
        frame: u64,
        in_flight: HashMap<TileKey, Task<LoadedTile>>,
        ready: HashMap<TileKey, (u64, LoadedTile)>,
        grounding: Option<TileCoord>,
    }

    impl TileResidency {
        /// Start a new assignment revision: cancel loads, drop held tiles, set grounding.
        pub fn reset(&mut self, revision: u64, grounding: Option<TileCoord>) {
            self.revision = revision;
            self.in_flight.clear();
            self.ready.clear();
            self.grounding = grounding;
        }

        /// The assignment revision this residency serves.
        pub fn revision(&self) -> u64 {
            self.revision
        }

        /// Tile whose elevation grounds the projection origin, while still unresolved.
        pub fn grounding(&self) -> Option<TileCoord> {
            self.grounding
        }

        /// Mark origin grounding resolved (grounded, or impossible).
        pub fn finish_grounding(&mut self) {
            self.grounding = None;
        }

        /// Harvest finished loads and expire held tiles nobody spawned; once per frame.
        pub fn poll(&mut self) {
            self.frame += 1;
            let frame = self.frame;
            let Self {
                in_flight, ready, ..
            } = self;
            in_flight.retain(|key, task| match check_ready(task) {
                Some(tile) => {
                    ready.insert(*key, (frame, tile));
                    false
                }
                None => true,
            });
            ready.retain(|_, (at, _)| frame.saturating_sub(*at) <= READY_TTL_FRAMES);
        }

        /// Resolve `coord` without blocking: ready bytes, a pending load, or absent.
        pub fn ensure_resident(
            &mut self,
            coord: TileCoord,
            composite: &CompositeTileSource,
        ) -> Residency {
            let key = key_of(coord);
            if let Some((_, tile)) = self.ready.remove(&key) {
                return classify(tile);
            }
            if self.in_flight.contains_key(&key) {
                return Residency::Pending;
            }
            if let Some(tile) = composite.resident_tile(coord) {
                return classify(tile);
            }
            if self.in_flight.len() >= MAX_TILE_LOADS_IN_FLIGHT {
                return Residency::Pending;
            }
            let loader = composite.loader();
            let pool = IoTaskPool::get_or_init(TaskPool::new);
            let task = pool.spawn(async move { loader.load_blocking(coord) });
            self.in_flight.insert(key, task);
            Residency::Pending
        }

        /// Return a tile taken by `ensure_resident` but not consumed (e.g. by grounding).
        pub fn put_back(&mut self, coord: TileCoord, tile: LoadedTile) {
            self.ready.insert(key_of(coord), (self.frame, tile));
        }

        /// Whether a load for `coord` is in flight.
        pub fn is_pending(&self, coord: TileCoord) -> bool {
            self.in_flight.contains_key(&key_of(coord))
        }

        /// Loads currently in flight.
        pub fn in_flight_len(&self) -> usize {
            self.in_flight.len()
        }

        /// Loaded tiles waiting to be spawned.
        pub fn ready_len(&self) -> usize {
            self.ready.len()
        }
    }

    fn classify(tile: LoadedTile) -> Residency {
        if tile.is_absent() {
            Residency::Absent
        } else {
            Residency::Ready(tile)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ByteLru;

    #[test]
    fn eviction_takes_the_least_recent_unpinned_entry_and_never_a_pinned_one() {
        // (operation, key, bytes) script over a 100-byte budget; expected survivors after each.
        let mut lru: ByteLru<&str, u8> = ByteLru::new(100);
        lru.insert("a", 1, 40);
        lru.insert("b", 2, 40);
        assert!(lru.pin(&"a"));
        let _ = lru.get(&"b"); // b most recent, but a is pinned anyway
        lru.insert("c", 3, 40); // 120 > 100: evicts the oldest UNPINNED = b
        assert!(lru.contains(&"a") && !lru.contains(&"b") && lru.contains(&"c"));
        assert_eq!(lru.total_bytes(), 80);

        lru.insert("d", 4, 90); // 170: c then d itself go; pinned a overshoots alone
        assert!(lru.contains(&"a") && !lru.contains(&"c"));
        assert_eq!(lru.total_bytes(), 40);
        assert_eq!(lru.pinned_len(), 1);
    }

    #[test]
    fn release_drops_bytes_only_when_the_last_pin_goes() {
        let mut lru: ByteLru<u32, u8> = ByteLru::new(1_000);
        lru.insert_pinned(7, 1, 10);
        lru.insert_pinned(7, 1, 10); // second holder of the same key
        lru.release(&7);
        assert!(lru.contains(&7), "one pin still holds it");
        lru.release(&7);
        assert!(!lru.contains(&7));
        assert_eq!(lru.total_bytes(), 0);

        lru.insert(1, 0, 5);
        lru.insert_pinned(2, 0, 5);
        lru.release_all_pins();
        assert!(lru.contains(&1) && !lru.contains(&2));
        lru.clear_unpinned();
        assert!(lru.is_empty());
        assert_eq!(lru.total_bytes(), 0);
    }
}
