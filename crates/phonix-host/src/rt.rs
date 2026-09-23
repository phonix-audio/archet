//! Runtime initialisation for low-latency audio on Linux.
//!
//! Call `init_rt_process` once at the start of `main`, before any thread,
//! in every binary that hosts an audio engine. It is best-effort: what the
//! process is not allowed to do is logged, and the binary runs on.
//!
//! ## What it does
//!
//! It locks the process's memory, so zram or swap never pages it out: a
//! page decompressed from zram stalls the audio thread for milliseconds,
//! even at SCHED_FIFO. Pages are locked as they are touched, so a reserved
//! range nobody uses (a thread's stack, a malloc arena) stays out of RAM.
//!
//! | memory-lock limit | what is locked |
//! |---|---|
//! | unlimited, or `CAP_IPC_LOCK` | every mapping, now and later |
//! | finite | what is mapped now |
//!
//! The kernel counts a locked mapping by its size, touched or not, and
//! refuses the mapping that would pass the limit. Under a finite limit,
//! locking later mappings would therefore refuse a thread's stack, a
//! plugin's library or a GPU driver's buffer once the process has mapped
//! the limit, and a plugin that cannot start a thread takes the host down.
//! So later mappings are locked only when nothing can refuse them.
//!
//! ## Full locking
//!
//! - `sudo setcap cap_ipc_lock+eip <binary>`: no re-login. The loader then
//!   refuses `LD_LIBRARY_PATH`, so `pw-jack` needs the libjack switch in
//!   `ld.so.conf.d` instead.
//! - `@audio - memlock unlimited` in `/etc/security/limits.d/`, then log
//!   out and back in.

/// The bit of `CAP_IPC_LOCK` in a capability mask.
#[cfg(target_os = "linux")]
const CAP_IPC_LOCK_BIT: u32 = 14;

/// Whether every later mapping can be locked without the kernel ever
/// refusing one: the limit is unlimited, or the process ignores it.
#[cfg(target_os = "linux")]
fn future_locking_is_unbounded() -> bool {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes one rlimit through a valid pointer.
    let unlimited = unsafe { libc::getrlimit(libc::RLIMIT_MEMLOCK, &mut limit) } == 0
        && limit.rlim_cur == libc::RLIM_INFINITY;
    unlimited || has_ipc_lock()
}

/// Whether the process's effective capabilities hold `CAP_IPC_LOCK`.
#[cfg(target_os = "linux")]
fn has_ipc_lock() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("CapEff:"))
                .and_then(|v| u64::from_str_radix(v.trim(), 16).ok())
        })
        .is_some_and(|mask| mask & (1 << CAP_IPC_LOCK_BIT) != 0)
}

#[cfg(target_os = "linux")]
pub fn init_rt_process(app_name: &str) {
    let future = future_locking_is_unbounded();
    let flags = libc::MCL_CURRENT | libc::MCL_ONFAULT | if future { libc::MCL_FUTURE } else { 0 };
    // SAFETY: mlockall takes flags and touches no Rust state.
    if unsafe { libc::mlockall(flags) } != 0 {
        log::warn!(
            "{app_name}: mlockall failed ({}); zram or swap may stall the audio thread under memory pressure. \
             Full locking: `sudo setcap cap_ipc_lock+eip <binary>`, or `@audio - memlock unlimited` \
             in /etc/security/limits.d/ and re-login.",
            std::io::Error::last_os_error()
        );
    } else if future {
        log::info!("{app_name}: memory locked, now and later");
    } else {
        log::warn!(
            "{app_name}: the memory-lock limit is finite, so only the memory mapped at start is locked; \
             plugins and threads loaded later can be paged out. Full locking: \
             `sudo setcap cap_ipc_lock+eip <binary>`, or `@audio - memlock unlimited` \
             in /etc/security/limits.d/ and re-login."
        );
    }
}

#[cfg(not(target_os = "linux"))]
pub fn init_rt_process(_app_name: &str) {
    // No-op on non-Linux. macOS has its own allocator + mach priorities;
    // Windows uses MMCSS via `audio_thread_priority`. Swap-stall jitter
    // isn't a comparable problem on either platform for the small working
    // sets our engines use.
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::os::unix::process::CommandExt;

    /// Set in the child process that runs the locked host.
    const CHILD: &str = "PHONIX_RT_LOCKED_CHILD";
    /// The child's memory-lock limit, far under what it maps.
    const LIMIT_BYTES: libc::rlim_t = 64 * 1024 * 1024;
    /// What the child maps past the limit, untouched.
    const MAP_BYTES: usize = 1 << 30;
    /// Threads the child starts, their stacks together past the limit.
    const THREADS: usize = 300;
    /// The test's own path, for the child to run it alone.
    const TEST_PATH: &str = "rt::tests::a_locked_host_maps_and_spawns_past_its_limit";

    /// A host locked under a finite limit still maps past the limit and
    /// starts threads past it. The host runs in a child process with the
    /// limit lowered, so the test process's memory is never locked.
    #[test]
    fn a_locked_host_maps_and_spawns_past_its_limit() {
        if std::env::var_os(CHILD).is_some() {
            super::init_rt_process("rt-test");
            let mut map = Vec::<u8>::new();
            if map.try_reserve_exact(MAP_BYTES).is_err() {
                eprintln!("the map past the limit was refused");
                std::process::exit(1);
            }
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS + 1));
            let mut handles = Vec::with_capacity(THREADS);
            for i in 0..THREADS {
                let barrier = barrier.clone();
                match std::thread::Builder::new().spawn(move || barrier.wait()) {
                    Ok(h) => handles.push(h),
                    Err(e) => {
                        eprintln!("thread {i} was refused: {e}");
                        std::process::exit(1);
                    }
                }
            }
            barrier.wait();
            for h in handles {
                let _ = h.join();
            }
            return;
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child.args(["--exact", TEST_PATH, "--test-threads=1", "--nocapture"]).env(CHILD, "1");
        // SAFETY: setrlimit is async-signal-safe and lowers only the
        // child's own limit, between fork and exec.
        unsafe {
            child.pre_exec(|| {
                let limit = libc::rlimit { rlim_cur: LIMIT_BYTES, rlim_max: LIMIT_BYTES };
                if libc::setrlimit(libc::RLIMIT_MEMLOCK, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let out = child.output().unwrap();
        assert!(
            out.status.success(),
            "the locked host failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
