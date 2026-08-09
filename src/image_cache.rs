use std::collections::VecDeque;

/// A small most-recently-used cache capped at a fixed size. Used by the Image Viewer to hold the
/// last few decoded/fitted bitmaps so re-visiting a nearby photo (Prev/Next, or a background
/// prefetch landing early) doesn't require a fresh decode from disk. Kept generic and independent
/// of NWG's `Bitmap` type so its eviction/recency logic is unit-testable without a window, per
/// `CLAUDE.md`'s testing rule.
pub struct ImageCache<K, T> {
    capacity: usize,
    /// Front = most recently used, back = least recently used (the next one evicted).
    entries: VecDeque<(K, T)>,
}

impl<K: PartialEq + Copy, T> ImageCache<K, T> {
    pub fn new(capacity: usize) -> Self {
        Self { capacity, entries: VecDeque::new() }
    }

    /// Looks up `key`, promoting it to most-recently-used on a hit so it survives eviction
    /// longer than entries nobody has revisited.
    pub fn get(&mut self, key: K) -> Option<&T> {
        let pos = self.entries.iter().position(|(k, _)| *k == key)?;
        if pos != 0 {
            let entry = self.entries.remove(pos)?;
            self.entries.push_front(entry);
        }
        self.entries.front().map(|(_, v)| v)
    }

    pub fn contains(&self, key: K) -> bool {
        self.entries.iter().any(|(k, _)| *k == key)
    }

    /// Inserts `value` as the most-recently-used entry, evicting the least-recently-used one(s)
    /// if that pushes the cache past capacity. A no-op if `key` is already present — callers
    /// check `contains`/`get` first, so this only ever runs for a genuine miss.
    pub fn insert(&mut self, key: K, value: T) {
        if self.contains(key) {
            return;
        }
        self.entries.push_front((key, value));
        while self.entries.len() > self.capacity {
            self.entries.pop_back();
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl<K, T> Default for ImageCache<K, T> {
    /// Capacity 0 — a placeholder until `ImageViewer::open` sets the real capacity, the same
    /// "Default now, real value assigned once the thread starts" pattern already used for its
    /// other `RefCell` fields (`images`, `source_dir`, ...).
    fn default() -> Self {
        Self { capacity: 0, entries: VecDeque::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_cache_is_empty() {
        let cache: ImageCache<usize, &str> = ImageCache::new(5);
        assert!(!cache.contains(1));
    }

    #[test]
    fn insert_then_get_returns_value() {
        let mut cache = ImageCache::new(5);
        cache.insert(1, "a");
        assert_eq!(cache.get(1), Some(&"a"));
    }

    #[test]
    fn get_moves_entry_to_front_protecting_it_from_eviction() {
        let mut cache = ImageCache::new(2);
        cache.insert(1, "a");
        cache.insert(2, "b");
        // Touch 1 so it becomes most-recently-used; 2 is now the one due for eviction.
        cache.get(1);
        cache.insert(3, "c");

        assert_eq!(cache.get(1), Some(&"a"));
        assert!(!cache.contains(2));
        assert_eq!(cache.get(3), Some(&"c"));
    }

    #[test]
    fn oldest_entry_evicted_when_capacity_exceeded() {
        let mut cache = ImageCache::new(2);
        cache.insert(1, "a");
        cache.insert(2, "b");
        cache.insert(3, "c");

        assert!(!cache.contains(1));
        assert!(cache.contains(2));
        assert!(cache.contains(3));
    }

    #[test]
    fn insert_duplicate_key_is_a_noop() {
        let mut cache = ImageCache::new(5);
        cache.insert(1, "a");
        cache.insert(1, "b");

        assert_eq!(cache.get(1), Some(&"a"));
    }

    #[test]
    fn clear_empties_the_cache() {
        let mut cache = ImageCache::new(5);
        cache.insert(1, "a");
        cache.clear();

        assert!(!cache.contains(1));
    }
}
