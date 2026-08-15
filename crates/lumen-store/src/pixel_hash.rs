//! Recent screenshot pixel-hash window — skip OCR on the same pixels.

use std::collections::{HashSet, VecDeque};

const DEFAULT_CAP: usize = 128;

/// Bounded set of recently seen `pixel_hash` values.
#[derive(Debug)]
pub struct PixelHashWindow {
    cap: usize,
    order: VecDeque<String>,
    set: HashSet<String>,
}

impl Default for PixelHashWindow {
    fn default() -> Self {
        Self::with_cap(DEFAULT_CAP)
    }
}

impl PixelHashWindow {
    pub fn with_cap(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            order: VecDeque::new(),
            set: HashSet::new(),
        }
    }

    pub fn contains(&self, hash: &str) -> bool {
        !hash.is_empty() && self.set.contains(hash)
    }

    /// Remember `hash` after we enqueued OCR for it.
    pub fn insert(&mut self, hash: &str) {
        if hash.is_empty() || self.set.contains(hash) {
            return;
        }
        while self.order.len() >= self.cap {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        self.set.insert(hash.to_string());
        self.order.push_back(hash.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_after_insert_and_evicts() {
        let mut w = PixelHashWindow::with_cap(2);
        assert!(!w.contains("dhash:aa"));
        w.insert("dhash:aa");
        w.insert("dhash:bb");
        assert!(w.contains("dhash:aa"));
        w.insert("dhash:cc");
        assert!(!w.contains("dhash:aa"));
        assert!(w.contains("dhash:bb"));
        assert!(w.contains("dhash:cc"));
    }

    #[test]
    fn empty_hash_is_never_remembered() {
        let mut w = PixelHashWindow::default();
        w.insert("");
        assert!(!w.contains(""));
    }
}
