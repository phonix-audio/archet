//! A value one thread edits and another runs, handed across without a lock
//! on the running side: the value is written before a counter moves, so a
//! reader that sees a new number reads the value that goes with it. Each
//! side remembers the number it last saw.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

pub struct Handoff<T> {
    live: RwLock<T>,
    rev: AtomicU64,
}

impl<T: Clone> Handoff<T> {
    pub fn new(value: T) -> Self {
        Handoff { live: RwLock::new(value), rev: AtomicU64::new(0) }
    }

    pub fn rev(&self) -> u64 {
        self.rev.load(Ordering::Acquire)
    }

    /// Replaces the value without moving the counter: what a side that has
    /// already applied `value` itself writes, so the other side finds it.
    pub fn seed(&self, value: T) {
        if let Ok(mut w) = self.live.write() {
            *w = value;
        }
    }

    /// Writes `value`, then moves the counter; returns the number the
    /// writer has now seen.
    pub fn publish(&self, value: T) -> u64 {
        if let Ok(mut w) = self.live.write() {
            *w = value;
        }
        self.rev.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// The editing side: a copy of the value when the counter moved since
    /// `seen`. May wait for the lock.
    pub fn adopt(&self, seen: &mut u64) -> Option<T> {
        let rev = self.rev();
        if rev == *seen {
            return None;
        }
        let value = self.live.read().ok()?.clone();
        *seen = rev;
        Some(value)
    }

    /// The running side: `f` on the value when the counter moved since
    /// `seen`, never waiting for the lock; a block missed is picked up on
    /// the next one. Returns whether `f` ran.
    pub fn apply_if_new(&self, seen: &mut u64, f: impl FnOnce(&T)) -> bool {
        let rev = self.rev();
        if rev == *seen {
            return false;
        }
        let Ok(value) = self.live.try_read() else { return false };
        f(&value);
        *seen = rev;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_publication_is_adopted_once_and_applied_once() {
        let h = Handoff::new(1);
        let (mut editor, mut audio) = (h.rev(), h.rev());
        assert!(h.adopt(&mut editor).is_none());
        audio = h.publish(2);
        assert_eq!(h.adopt(&mut editor), Some(2));
        assert!(h.adopt(&mut editor).is_none());
        assert!(!h.apply_if_new(&mut audio, |_| panic!("its own publication")));
        editor = h.publish(3);
        let mut got = 0;
        assert!(h.apply_if_new(&mut audio, |v| got = *v));
        assert_eq!(got, 3);
        assert!(!h.apply_if_new(&mut audio, |_| unreachable!()));
        assert_eq!(editor, h.rev());
    }

    #[test]
    fn a_seed_is_found_without_a_move() {
        let h = Handoff::new(0);
        let mut seen = h.rev();
        h.seed(7);
        assert!(h.adopt(&mut seen).is_none());
        let n = h.publish(8);
        assert_eq!(h.adopt(&mut seen), Some(8));
        assert_eq!(seen, n);
    }
}
