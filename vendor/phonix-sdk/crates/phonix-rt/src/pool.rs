//! A pool of workers for an audio thread: the same job over every item of
//! a slice, each item on one thread, with the caller working too.
//!
//! The workers are threads that live as long as the pool. Between jobs
//! they park on a condition variable, so an idle pool costs nothing; inside
//! a job they take items from a shared counter and spin at the end until
//! the last item is done, since a job lasts a fraction of a block and a
//! futex round trip would eat it. The caller publishes the job, takes items
//! itself, and returns when every item is done: the slice is borrowed
//! mutably for exactly that long, and no two threads ever hold the same
//! item, which is what makes handing the workers a raw pointer sound.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

/// What the workers see of a job: the slice as a pointer, its length, and
/// the function to run, all valid until `remaining` reaches zero.
struct Job {
    items: *mut u8,
    len: usize,
    run: *const (dyn Fn(usize, *mut u8) + Sync),
}

// The pointers are only ever dereferenced inside `run`, on an index no
// other thread holds, while the caller is blocked in `run_over`.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

struct Shared {
    /// Bumped for every job; a worker wakes when it changes.
    generation: Mutex<u64>,
    wake: Condvar,
    job: Mutex<Option<Job>>,
    /// Items handed out so far, and items still being worked on.
    next: AtomicUsize,
    remaining: AtomicUsize,
    stop: AtomicBool,
}

/// The pool: its workers, and the caller's share of the work.
pub struct Pool {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl Pool {
    /// A pool of up to `workers` threads beside the caller, as many as the
    /// system grants. Zero workers is a pool that runs everything on the
    /// caller, which is what a single core gets.
    pub fn new(workers: usize) -> Self {
        let shared = Arc::new(Shared {
            generation: Mutex::new(0),
            wake: Condvar::new(),
            job: Mutex::new(None),
            next: AtomicUsize::new(0),
            remaining: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        });
        // A worker the system will not make is a worker fewer, never a
        // panic: a host that locks its memory or caps its threads can
        // refuse one, and a panic here takes the host down with the plugin.
        let mut handles = Vec::with_capacity(workers);
        for i in 0..workers {
            let shared = Arc::clone(&shared);
            match std::thread::Builder::new().name(format!("phonix-pool-{i}")).spawn(move || worker(&shared)) {
                Ok(h) => handles.push(h),
                Err(_) => break,
            }
        }
        Self { shared, workers: handles }
    }

    /// How many threads work a job, the caller included.
    pub fn threads(&self) -> usize {
        self.workers.len() + 1
    }

    /// Run `f` over every item of `items`, once each, on the caller and the
    /// workers together, and return when the last one is done.
    pub fn run_over<T: Send>(&self, items: &mut [T], f: impl Fn(usize, &mut T) + Sync) {
        if items.is_empty() {
            return;
        }
        if self.workers.is_empty() || items.len() == 1 {
            for (i, item) in items.iter_mut().enumerate() {
                f(i, item);
            }
            return;
        }
        // The job as the workers see it: the function boxed behind a thin
        // trampoline that turns the byte pointer back into the item.
        let run = |i: usize, base: *mut u8| {
            // SAFETY: `base` is `items.as_mut_ptr()` cast, `i` is below
            // `items.len()` and handed to one thread only.
            let item = unsafe { &mut *(base as *mut T).add(i) };
            f(i, item);
        };
        let run_ref: &(dyn Fn(usize, *mut u8) + Sync) = &run;
        // SAFETY: the lifetime is erased for the job's crossing to the
        // workers; the caller does not return before every item is done,
        // so `run` and `items` outlive every use.
        let run_static: *const (dyn Fn(usize, *mut u8) + Sync) =
            unsafe { std::mem::transmute(run_ref as *const (dyn Fn(usize, *mut u8) + Sync)) };
        let shared = &self.shared;
        shared.next.store(0, Ordering::Relaxed);
        shared.remaining.store(items.len(), Ordering::Relaxed);
        *shared.job.lock().unwrap() = Some(Job { items: items.as_mut_ptr() as *mut u8, len: items.len(), run: run_static });
        {
            let mut g = shared.generation.lock().unwrap();
            *g += 1;
        }
        shared.wake.notify_all();
        // The caller works too.
        take_items(shared, items.as_mut_ptr() as *mut u8, items.len(), run_ref);
        while shared.remaining.load(Ordering::Acquire) != 0 {
            std::hint::spin_loop();
        }
        *shared.job.lock().unwrap() = None;
    }
}

/// Take items off the counter until none is left, running each.
fn take_items(shared: &Shared, base: *mut u8, len: usize, run: &(dyn Fn(usize, *mut u8) + Sync)) {
    loop {
        let i = shared.next.fetch_add(1, Ordering::AcqRel);
        if i >= len {
            return;
        }
        run(i, base);
        shared.remaining.fetch_sub(1, Ordering::AcqRel);
    }
}

fn worker(shared: &Shared) {
    let mut seen = 0u64;
    loop {
        {
            let mut g = shared.generation.lock().unwrap();
            while *g == seen && !shared.stop.load(Ordering::Relaxed) {
                g = shared.wake.wait(g).unwrap();
            }
            seen = *g;
        }
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        let job = {
            let j = shared.job.lock().unwrap();
            j.as_ref().map(|j| (j.items, j.len, j.run))
        };
        if let Some((items, len, run)) = job {
            // SAFETY: the caller keeps the job alive until `remaining` is
            // zero, and it cannot reach zero before this worker's items are
            // done.
            let run: &(dyn Fn(usize, *mut u8) + Sync) = unsafe { &*run };
            take_items(shared, items, len, run);
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        {
            let _g = self.shared.generation.lock().unwrap();
        }
        self.shared.wake.notify_all();
        for h in self.workers.drain(..) {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every item is visited once, whatever the pool's size.
    #[test]
    fn every_item_once() {
        for workers in [0, 1, 3, 7] {
            let pool = Pool::new(workers);
            let mut items: Vec<u32> = vec![0; 1000];
            for _ in 0..50 {
                pool.run_over(&mut items, |i, x| *x += i as u32 + 1);
            }
            for (i, x) in items.iter().enumerate() {
                assert_eq!(*x, 50 * (i as u32 + 1), "item {i} with {workers} workers");
            }
        }
    }

    /// The work is really shared: with workers, more than one thread is
    /// seen inside the job.
    #[test]
    fn the_workers_work() {
        let pool = Pool::new(3);
        let seen = Mutex::new(std::collections::HashSet::new());
        let mut items = vec![0u8; 400];
        pool.run_over(&mut items, |_, _| {
            seen.lock().unwrap().insert(std::thread::current().id());
            std::thread::sleep(std::time::Duration::from_micros(50));
        });
        assert!(seen.lock().unwrap().len() > 1);
    }
}
