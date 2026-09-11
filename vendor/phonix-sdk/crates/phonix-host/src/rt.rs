//! Runtime initialisation for low-latency audio on Linux.
//!
//! Call `init_rt_process` once at the start of `main` in every binary that
//! hosts an audio engine. It does two things, both best-effort — if the
//! process lacks the necessary capabilities, each call fails with a warning
//! in the log and the binary continues to run correctly, just without the
//! latency protections.
//!
//! ## What it does
//!
//! 1. **`mlockall(MCL_CURRENT | MCL_FUTURE)`** — pins every currently-mapped
//!    page of this process into RAM and keeps every future mapping pinned
//!    too. On systems using zram as swap (the default on many modern Linux
//!    distros, and what we have on this workstation) this stops the kernel
//!    from compressing our pages into zram. Decompressing a zram page can
//!    take several milliseconds in a kernel section with preemption
//!    disabled, which blocks the audio thread **even at SCHED_FIFO 80**. We
//!    measured this first-hand: 100–500 ms scheduling gaps while the drum
//!    plugin window was open, disappearing after `swapoff /dev/zram0`.
//!
//! 2. **Diagnostic output** — logs exactly which protections took effect
//!    and which failed, so the user can see at a glance whether the
//!    process is running with full low-latency guarantees.
//!
//! ## Required capabilities
//!
//! `mlockall` obeys `RLIMIT_MEMLOCK`, which defaults to 64 KiB — vastly
//! too small for pinning a full process image. Three ways to make it work:
//!
//! - **CAP_IPC_LOCK** on the binary itself (recommended, no login session
//!   restart needed):
//!   ```text
//!   sudo setcap cap_ipc_lock+eip target/debug/sequencer
//!   ```
//!   This grants the binary the capability regardless of the user's ulimit.
//!   Note: the loader refuses `LD_LIBRARY_PATH` on binaries with elevated
//!   caps, so if you use `pw-jack` (which sets `LD_LIBRARY_PATH`) you'll
//!   need the permanent libjack switch via `ld.so.conf.d` instead.
//!
//! - **PAM limits** — drop a file in `/etc/security/limits.d/` with
//!   `@audio - memlock unlimited` and log out / back in. This is the
//!   pro-audio distro standard setup.
//!
//! - **Run as root** — works but not recommended for a user app.

#[cfg(target_os = "linux")]
pub fn init_rt_process(app_name: &str) {
    use std::io::Error;

    // SAFETY: `mlockall` is a simple syscall; no Rust-level invariants
    // involved. `MCL_CURRENT | MCL_FUTURE` is the standard pro-audio flag
    // combination that pins everything existing and requests all future
    // mappings to be pinned as well.
    let ret = unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };
    if ret == 0 {
        log::info!(
            "{}: mlockall(MCL_CURRENT | MCL_FUTURE) succeeded — all pages pinned in RAM, zram/swap will not touch this process",
            app_name
        );
    } else {
        let err = Error::last_os_error();
        log::warn!(
            "{}: mlockall failed ({}) — zram or swap may stall the audio thread under memory pressure. \
             To fix: `sudo setcap cap_ipc_lock+eip <binary>` (no re-login needed), \
             or add `@audio - memlock unlimited` to /etc/security/limits.d/ and re-login.",
            app_name,
            err
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
