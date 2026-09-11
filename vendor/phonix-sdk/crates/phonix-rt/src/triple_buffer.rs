use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Bit layout of the `shared` atomic:
///
/// ```text
///   bit 0-1 : index (0..2) of the currently-published slot
///   bit 2   : "new data" flag — set by writer on publish, cleared by reader
/// ```
const IDX_MASK: u8 = 0b011;
const DIRTY_BIT: u8 = 0b100;

/// Inner shared state for a triple buffer. Holds three slots and one atomic.
pub struct TripleBuffer<T> {
    slots: [UnsafeCell<T>; 3],
    shared: AtomicU8,
}

// SAFETY: The only way to access slots is via `Writer`/`Reader`, which are
// handed out in a non-Clone split and each own a distinct private index.
// The shared atomic ensures those indices never collide. `T: Send` is
// required so the slot's data can be sent across thread boundaries.
unsafe impl<T: Send> Send for TripleBuffer<T> {}
unsafe impl<T: Send> Sync for TripleBuffer<T> {}

impl<T: Default> TripleBuffer<T> {
    /// Construct a fresh triple buffer with three default-initialised slots
    /// and split it into a `(Writer, Reader)` pair. Initial assignment:
    /// writer-private = 0, reader-private = 1, shared = 2, no dirty flag.
    pub fn new() -> (Writer<T>, Reader<T>) {
        let buf = Arc::new(Self {
            slots: [
                UnsafeCell::new(T::default()),
                UnsafeCell::new(T::default()),
                UnsafeCell::new(T::default()),
            ],
            shared: AtomicU8::new(2 & IDX_MASK),
        });
        (
            Writer {
                buf: buf.clone(),
                private_idx: 0,
            },
            Reader {
                buf,
                private_idx: 1,
            },
        )
    }
}

/// Write handle. Not `Clone` — there is exactly one writer.
pub struct Writer<T> {
    buf: Arc<TripleBuffer<T>>,
    private_idx: u8,
}

// SAFETY: Writer holds exclusive access to its `private_idx` slot; the
// invariant is maintained by the swap in `publish`. Sending the writer to
// another thread is fine because the invariant is thread-local to whichever
// thread currently holds it.
unsafe impl<T: Send> Send for Writer<T> {}

impl<T> Writer<T> {
    /// Mutable access to the writer's private slot. The reader can never
    /// see this slot while this reference lives.
    #[inline]
    pub fn edit(&mut self) -> &mut T {
        // SAFETY: Only the writer ever accesses `private_idx`. The atomic
        // swap in `publish` is the single synchronisation point, and
        // between publishes the slot is writer-exclusive.
        unsafe { &mut *self.buf.slots[self.private_idx as usize].get() }
    }

    /// Atomically publish the current private slot. After this call the
    /// writer owns a different slot (whatever was previously published) and
    /// may start overwriting it immediately.
    #[inline]
    pub fn publish(&mut self) {
        let new_shared = (self.private_idx & IDX_MASK) | DIRTY_BIT;
        let prev = self.buf.shared.swap(new_shared, Ordering::AcqRel);
        self.private_idx = prev & IDX_MASK;
    }
}

/// Read handle. Not `Clone` — there is exactly one reader.
pub struct Reader<T> {
    buf: Arc<TripleBuffer<T>>,
    private_idx: u8,
}

unsafe impl<T: Send> Send for Reader<T> {}

impl<T> Reader<T> {
    /// Check if the writer has published new data since the last `read`. If
    /// so, swap our private slot with the latest-published one and return a
    /// reference into the now-reader-owned slot. If not, return `None`.
    ///
    /// The returned reference borrows from `self`, so the reader can do
    /// whatever it wants with it before the next `read` call — the writer
    /// cannot touch the slot while the borrow is alive.
    #[inline]
    pub fn read(&mut self) -> Option<&T> {
        // Fast path: no new data → avoid the RMW.
        let cur = self.buf.shared.load(Ordering::Acquire);
        if cur & DIRTY_BIT == 0 {
            return None;
        }
        // Consume: swap our private index into the shared slot, taking
        // ownership of whatever was published. Clear the dirty bit so the
        // next read returns `None` unless the writer has published again.
        let new_shared = self.private_idx & IDX_MASK;
        let prev = self.buf.shared.swap(new_shared, Ordering::AcqRel);
        self.private_idx = prev & IDX_MASK;
        // SAFETY: After the swap, `private_idx` names a slot the writer
        // cannot touch until we hand it back on the next `read`. The
        // returned reference is bound to `&mut self`, so the borrow checker
        // enforces that we don't re-enter `read` while it's alive.
        Some(unsafe { &*self.buf.slots[self.private_idx as usize].get() })
    }

    /// Peek at the reader-owned slot without checking for new data. Useful
    /// when the caller already consumed the last publication with `read`
    /// and wants to reuse the cached value on a subsequent frame.
    #[inline]
    pub fn cached(&self) -> &T {
        unsafe { &*self.buf.slots[self.private_idx as usize].get() }
    }
}

/// Shared reader handle used for meter state on the GUI side. Wraps a
/// `Reader` in an `Arc<Mutex<>>` so the same reader can be referenced by
/// multiple GUI panels (e.g., main view + plugin window). The mutex is
/// only ever held by GUI threads — the audio thread owns the Writer
/// directly, no sharing.
pub type SharedReader<T> = std::sync::Arc<std::sync::Mutex<Reader<T>>>;

/// Convenience constructor: build a triple buffer and return the writer
/// plus a `SharedReader` ready to be cloned to GUI panels.
pub fn meter_channel<T: Default>() -> (Writer<T>, SharedReader<T>) {
    let (w, r) = TripleBuffer::<T>::new();
    (w, std::sync::Arc::new(std::sync::Mutex::new(r)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_read_roundtrip() {
        let (mut w, mut r) = TripleBuffer::<u32>::new();
        assert!(r.read().is_none(), "fresh buffer reports no data");

        *w.edit() = 42;
        w.publish();
        assert_eq!(r.read().copied(), Some(42));
        assert!(r.read().is_none(), "stale after consume");

        *w.edit() = 7;
        w.publish();
        *w.edit() = 99;
        w.publish();
        // Reader sees the latest publication only, not the intermediate.
        assert_eq!(r.read().copied(), Some(99));
    }

    #[test]
    fn slot_indices_stay_distinct() {
        let (mut w, mut r) = TripleBuffer::<u32>::new();
        let mut seen = std::collections::HashSet::new();
        seen.insert((w.private_idx, r.private_idx));
        for _ in 0..32 {
            w.publish();
            let _ = r.read();
            // Writer, reader and shared must all be distinct.
            let shared = r.buf.shared.load(Ordering::Relaxed) & IDX_MASK;
            assert_ne!(w.private_idx, r.private_idx);
            assert_ne!(w.private_idx, shared);
            assert_ne!(r.private_idx, shared);
        }
    }
}
