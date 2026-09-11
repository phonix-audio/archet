//! JACK backend — native JACK client used on Linux.
//!
//! Works transparently against either `jackd` or `pipewire-jack`, which means
//! PipeWire users get a native JACK-API path instead of going through the
//! ALSA compatibility shim that `cpal` is stuck on.
//!
//! The process handler keeps a pre-sized interleaved scratch buffer so each
//! cycle does zero heap allocation: the engine callback fills it, and we
//! de-interleave into JACK's planar output ports in place. For duplex we do
//! the reverse on the capture side before calling the input callback.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use jack::{
    AudioIn, AudioOut, Client, ClientOptions, Control, NotificationHandler, Port, ProcessHandler,
    ProcessScope,
};

use super::{AudioStream, AudioStreamConfig, InputCallback, OpenHint, OutputCallback};

/// Stats shared between the audio thread (producer, lock-free writes) and a
/// dedicated logger thread (consumer). Every field is an atomic so the
/// audio thread never blocks on a mutex, allocates, or makes a syscall —
/// `log::info!` used to be called directly from the process handler, which
/// meant a `write(2)` to stderr on the audio thread once per second and
/// was itself a significant xrun source.
#[derive(Default)]
struct SharedStats {
    /// Cumulative xrun count — incremented from the JACK notification thread.
    xruns: AtomicU64,
    /// Cumulative cycle count since stream start.
    cycles: AtomicU64,
    /// Cumulative nanoseconds spent inside the process callback.
    total_ns: AtomicU64,
    /// Maximum single-cycle duration since last reset. The logger thread
    /// reads-and-swaps this to extract per-window max.
    max_ns: AtomicU64,
    /// Set to true by the logger thread when the AudioStream is dropped, so
    /// the logger's sleep loop can exit cleanly.
    stop: std::sync::atomic::AtomicBool,
}

/// Notification handler: runs on JACK's own notification thread, bumps the
/// xrun counter. Lock-free.
struct XrunNotifier {
    stats: Arc<SharedStats>,
}

impl NotificationHandler for XrunNotifier {
    fn xrun(&mut self, _: &Client) -> Control {
        self.stats.xruns.fetch_add(1, Ordering::Relaxed);
        Control::Continue
    }
}

/// Writer-side load recorder. All updates are non-blocking atomic writes —
/// the audio thread never calls `log::`, never allocates, never blocks.
struct LoadMeter {
    stats: Arc<SharedStats>,
}

impl LoadMeter {
    fn new(stats: Arc<SharedStats>) -> Self {
        Self { stats }
    }

    #[inline]
    fn record(&self, elapsed_ns: u64) {
        self.stats.cycles.fetch_add(1, Ordering::Relaxed);
        self.stats.total_ns.fetch_add(elapsed_ns, Ordering::Relaxed);
        // Lock-free max: swap in the new value if it beats the old.
        let mut cur = self.stats.max_ns.load(Ordering::Relaxed);
        while elapsed_ns > cur {
            match self.stats.max_ns.compare_exchange_weak(
                cur,
                elapsed_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => cur = observed,
            }
        }
    }
}

/// Spawn a background thread that samples `SharedStats` every second and
/// emits the log line we used to emit from the audio thread. This is where
/// the `write(2)` to stderr happens — on a normal thread, not the JACK
/// data-loop. Handle joins on drop via the `stop` flag.
fn spawn_stats_logger(
    stats: Arc<SharedStats>,
    sample_rate: u32,
    buffer_frames: u32,
    tag: &'static str,
) -> std::thread::JoinHandle<()> {
    let budget_ns: u64 =
        (buffer_frames as u64 * 1_000_000_000) / sample_rate.max(1) as u64;
    let budget_ns = budget_ns.max(1);

    std::thread::Builder::new()
        .name(format!("audio-stats-{}", tag))
        .spawn(move || {
            let mut last_cycles: u64 = 0;
            let mut last_total_ns: u64 = 0;
            let mut last_xruns: u64 = 0;
            while !stats.stop.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if stats.stop.load(Ordering::Relaxed) {
                    break;
                }

                let cycles = stats.cycles.load(Ordering::Relaxed);
                let total_ns = stats.total_ns.load(Ordering::Relaxed);
                let xruns = stats.xruns.load(Ordering::Relaxed);
                // Read-and-reset max for this window.
                let max_ns = stats.max_ns.swap(0, Ordering::Relaxed);

                let d_cycles = cycles.saturating_sub(last_cycles);
                let d_total_ns = total_ns.saturating_sub(last_total_ns);
                let d_xruns = xruns.saturating_sub(last_xruns);
                last_cycles = cycles;
                last_total_ns = total_ns;
                last_xruns = xruns;

                if d_cycles == 0 {
                    continue;
                }

                let avg_ns = d_total_ns / d_cycles;
                let avg_pct = (avg_ns * 100) / budget_ns;
                let max_pct = (max_ns * 100) / budget_ns;

                // Warn about what actually went wrong, and only that. A block
                // that overran, or a cycle the backend could not deliver, is a
                // fault; half the budget used is a piano playing.
                if d_xruns > 0 || max_pct >= 100 {
                    log::warn!(
                        "JACK [{}] load avg {:>3}% / max {:>3}% | xruns +{} (total {}) | {} cycles/s",
                        tag,
                        avg_pct,
                        max_pct,
                        d_xruns,
                        xruns,
                        d_cycles
                    );
                } else if avg_pct >= 50 || max_pct >= 80 {
                    log::info!(
                        "JACK [{}] load avg {:>3}% / max {:>3}% | {} cycles/s",
                        tag,
                        avg_pct,
                        max_pct,
                        d_cycles
                    );
                } else {
                    log::debug!(
                        "JACK [{}] load avg {:>3}% / max {:>3}% | {} cycles/s",
                        tag,
                        avg_pct,
                        max_pct,
                        d_cycles
                    );
                }
            }
        })
        .expect("spawn audio stats logger")
}

/// RAII wrapper: holds the live JACK client and the logger thread, joins
/// the logger on drop. Returned to the caller as `AudioStream` contents.
struct JackStreamBundle<N: NotificationHandler + 'static, P: ProcessHandler + 'static> {
    _client: jack::AsyncClient<N, P>,
    stats: Arc<SharedStats>,
    logger: Option<std::thread::JoinHandle<()>>,
}

impl<N: NotificationHandler + 'static, P: ProcessHandler + 'static> Drop
    for JackStreamBundle<N, P>
{
    fn drop(&mut self) {
        self.stats.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.logger.take() {
            let _ = h.join();
        }
    }
}

/// Upper bound on the JACK buffer size (frames) we'll ever see. Pre-allocating
/// the interleave scratch buffer to this size keeps the process callback
/// allocation-free. JACK servers in the wild use 64..2048; 16384 is a
/// comfortable ceiling that still costs well under 1 MiB of RAM.
const MAX_JACK_FRAMES: usize = 16384;

/// Try to connect to a running JACK server. Returns `Err` if no server is
/// reachable (caller then falls back to cpal). Uses `NO_START_SERVER` so we
/// never try to spawn `jackd` on the user's behalf.
pub(super) fn probe_client(hint: &OpenHint) -> std::result::Result<Client, String> {
    match Client::new(hint.app_name, ClientOptions::NO_START_SERVER) {
        Ok((client, _status)) => Ok(client),
        Err(e) => Err(format!("{:?}", e)),
    }
}

/// Output-only process handler. Engine callback writes interleaved stereo
/// into `scratch`; we split it out into the two JACK output ports.
struct OutputHandler {
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    cb: Box<dyn OutputCallback>,
    scratch: Vec<f32>,
    rt_tried: bool,
    rt_sample_rate: u32,
    #[allow(dead_code)]
    rt_handle: Option<audio_thread_priority::RtPriorityHandle>,
    meter: LoadMeter,
}

impl ProcessHandler for OutputHandler {
    fn process(&mut self, _: &Client, ps: &ProcessScope) -> Control {
        // Per-thread, sticky and free; set here because the audio thread is
        // JACK's, created outside our control.
        phonix_rt::denormal::enable_flush_to_zero();
        if !self.rt_tried {
            self.rt_tried = true;
            self.rt_handle = audio_thread_priority::promote_current_thread_to_real_time(
                ps.n_frames(),
                self.rt_sample_rate,
            )
            .map(Some)
            .unwrap_or_else(|e| {
                log::warn!("JACK: could not promote audio thread to RT: {:?}", e);
                None
            });
            log::info!(
                "JACK audio thread running at {} Hz, {} frames/cycle",
                self.rt_sample_rate,
                ps.n_frames()
            );
        }

        let t0 = Instant::now();

        let frames = ps.n_frames() as usize;
        debug_assert!(frames * 2 <= self.scratch.len());
        let slice = &mut self.scratch[..frames * 2];
        for s in slice.iter_mut() {
            *s = 0.0;
        }
        self.cb.process(slice, 2);

        let out_l = self.out_l.as_mut_slice(ps);
        let out_r = self.out_r.as_mut_slice(ps);
        for i in 0..frames {
            out_l[i] = slice[i * 2];
            out_r[i] = slice[i * 2 + 1];
        }

        self.meter.record(t0.elapsed().as_nanos() as u64);
        Control::Continue
    }
}

/// Duplex process handler. Capture ports feed an input scratch (interleaved
/// stereo) which is handed to the input callback; the output callback then
/// fills an output scratch that is split out into playback ports.
struct DuplexHandler {
    in_l: Port<AudioIn>,
    in_r: Port<AudioIn>,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    out_cb: Box<dyn OutputCallback>,
    in_cb: Box<dyn InputCallback>,
    out_scratch: Vec<f32>,
    in_scratch: Vec<f32>,
    rt_tried: bool,
    rt_sample_rate: u32,
    #[allow(dead_code)]
    rt_handle: Option<audio_thread_priority::RtPriorityHandle>,
    meter: LoadMeter,
}

impl ProcessHandler for DuplexHandler {
    fn process(&mut self, _: &Client, ps: &ProcessScope) -> Control {
        // Per-thread, sticky and free; set here because the audio thread is
        // JACK's, created outside our control.
        phonix_rt::denormal::enable_flush_to_zero();
        if !self.rt_tried {
            self.rt_tried = true;
            self.rt_handle = audio_thread_priority::promote_current_thread_to_real_time(
                ps.n_frames(),
                self.rt_sample_rate,
            )
            .map(Some)
            .unwrap_or_else(|e| {
                log::warn!("JACK: could not promote audio thread to RT: {:?}", e);
                None
            });
        }

        let t0 = Instant::now();
        let frames = ps.n_frames() as usize;
        debug_assert!(frames * 2 <= self.out_scratch.len());
        debug_assert!(frames * 2 <= self.in_scratch.len());

        // Capture → interleave → input callback.
        {
            let in_l = self.in_l.as_slice(ps);
            let in_r = self.in_r.as_slice(ps);
            let in_buf = &mut self.in_scratch[..frames * 2];
            for i in 0..frames {
                in_buf[i * 2] = in_l[i];
                in_buf[i * 2 + 1] = in_r[i];
            }
            self.in_cb.process(in_buf, 2);
        }

        // Output callback → de-interleave → playback.
        {
            let out_buf = &mut self.out_scratch[..frames * 2];
            for s in out_buf.iter_mut() {
                *s = 0.0;
            }
            self.out_cb.process(out_buf, 2);

            let out_l = self.out_l.as_mut_slice(ps);
            let out_r = self.out_r.as_mut_slice(ps);
            for i in 0..frames {
                out_l[i] = out_buf[i * 2];
                out_r[i] = out_buf[i * 2 + 1];
            }
        }

        self.meter.record(t0.elapsed().as_nanos() as u64);
        Control::Continue
    }
}

/// The physical ports belonging to client `group`, in the server's order.
///
/// Matched by prefix rather than through the server's regex filter: a port
/// name carries parentheses, slashes and dots that a regex would read as
/// syntax.
fn physical_ports(client: &Client, group: &str, flags: jack::PortFlags) -> Vec<String> {
    let prefix = format!("{group}:");
    client
        .ports(None, None, flags | jack::PortFlags::IS_PHYSICAL)
        .into_iter()
        .filter(|p| p.starts_with(&prefix))
        .collect()
}

fn try_autoconnect_output(client: &Client, our_l: &str, our_r: &str, want: Option<&str>) {
    let mut playback = Vec::new();
    if let Some(group) = want {
        playback = physical_ports(client, group, jack::PortFlags::IS_INPUT);
        if playback.len() < 2 {
            log::warn!(
                "JACK: output '{}' has {} playback port(s); using the default instead",
                group,
                playback.len()
            );
            playback.clear();
        }
    }
    if playback.is_empty() {
        playback = client.ports(
            Some("system:playback_.*"),
            None,
            jack::PortFlags::IS_INPUT,
        );
    }
    if playback.len() >= 2 {
        if let Err(e) = client.connect_ports_by_name(our_l, &playback[0]) {
            log::warn!("JACK: failed to auto-connect L → {}: {:?}", playback[0], e);
        }
        if let Err(e) = client.connect_ports_by_name(our_r, &playback[1]) {
            log::warn!("JACK: failed to auto-connect R → {}: {:?}", playback[1], e);
        }
    } else {
        log::info!(
            "JACK: no system playback ports to auto-connect to (found {})",
            playback.len()
        );
    }
}

fn try_autoconnect_input(client: &Client, our_l: &str, our_r: &str, want: Option<&str>) {
    let mut capture = Vec::new();
    if let Some(group) = want {
        capture = physical_ports(client, group, jack::PortFlags::IS_OUTPUT);
        if capture.is_empty() {
            log::warn!("JACK: input '{}' has no capture port; using the default", group);
        }
    }
    if capture.is_empty() {
        capture = client.ports(
            Some("system:capture_.*"),
            None,
            jack::PortFlags::IS_OUTPUT,
        );
    }
    if capture.len() >= 2 {
        if let Err(e) = client.connect_ports_by_name(&capture[0], our_l) {
            log::warn!("JACK: failed to auto-connect capture L: {:?}", e);
        }
        if let Err(e) = client.connect_ports_by_name(&capture[1], our_r) {
            log::warn!("JACK: failed to auto-connect capture R: {:?}", e);
        }
    } else if let Some(first) = capture.first() {
        // Mono capture → feed both of our input ports from the same source.
        let _ = client.connect_ports_by_name(first, our_l);
        let _ = client.connect_ports_by_name(first, our_r);
    }
}

pub(super) fn finish_open_output<F>(
    client: Client,
    hint: &super::OpenHint,
    make_cb: F,
) -> Result<(AudioStream, AudioStreamConfig)>
where
    F: FnOnce(&AudioStreamConfig) -> Box<dyn OutputCallback>,
{
    let sr = client.sample_rate() as f32;
    let buffer_frames = client.buffer_size();
    let device_name = client.name().to_string();

    let cfg = AudioStreamConfig {
        sample_rate: sr,
        channels: 2,
        buffer_frames,
        backend_name: "jack",
        device_name,
    };

    let out_l = client
        .register_port("out_L", AudioOut::default())
        .map_err(|e| anyhow::anyhow!("JACK register out_L: {:?}", e))?;
    let out_r = client
        .register_port("out_R", AudioOut::default())
        .map_err(|e| anyhow::anyhow!("JACK register out_R: {:?}", e))?;
    let our_l_name = out_l.name().map_err(|e| anyhow::anyhow!("{:?}", e))?;
    let our_r_name = out_r.name().map_err(|e| anyhow::anyhow!("{:?}", e))?;

    let cb = make_cb(&cfg);

    let stats = Arc::new(SharedStats::default());
    let handler = OutputHandler {
        out_l,
        out_r,
        cb,
        scratch: vec![0.0; MAX_JACK_FRAMES * 2],
        rt_tried: false,
        rt_sample_rate: sr as u32,
        rt_handle: None,
        meter: LoadMeter::new(stats.clone()),
    };

    let active = client
        .activate_async(
            XrunNotifier {
                stats: stats.clone(),
            },
            handler,
        )
        .map_err(|e| anyhow::anyhow!("JACK activate: {:?}", e))?;

    try_autoconnect_output(active.as_client(), &our_l_name, &our_r_name, hint.output_device.as_deref());

    let logger = spawn_stats_logger(stats.clone(), sr as u32, buffer_frames, "out");
    let bundle = JackStreamBundle {
        _client: active,
        stats,
        logger: Some(logger),
    };
    Ok((AudioStream::new(bundle), cfg))
}

pub(super) fn finish_open_duplex<F>(
    client: Client,
    hint: &super::OpenHint,
    make_cbs: F,
) -> Result<(AudioStream, AudioStreamConfig)>
where
    F: FnOnce(&AudioStreamConfig) -> (Box<dyn OutputCallback>, Box<dyn InputCallback>),
{
    let sr = client.sample_rate() as f32;
    let buffer_frames = client.buffer_size();
    let device_name = client.name().to_string();

    let cfg = AudioStreamConfig {
        sample_rate: sr,
        channels: 2,
        buffer_frames,
        backend_name: "jack",
        device_name,
    };

    let in_l = client
        .register_port("in_L", AudioIn::default())
        .map_err(|e| anyhow::anyhow!("JACK register in_L: {:?}", e))?;
    let in_r = client
        .register_port("in_R", AudioIn::default())
        .map_err(|e| anyhow::anyhow!("JACK register in_R: {:?}", e))?;
    let out_l = client
        .register_port("out_L", AudioOut::default())
        .map_err(|e| anyhow::anyhow!("JACK register out_L: {:?}", e))?;
    let out_r = client
        .register_port("out_R", AudioOut::default())
        .map_err(|e| anyhow::anyhow!("JACK register out_R: {:?}", e))?;

    let our_in_l = in_l.name().map_err(|e| anyhow::anyhow!("{:?}", e))?;
    let our_in_r = in_r.name().map_err(|e| anyhow::anyhow!("{:?}", e))?;
    let our_out_l = out_l.name().map_err(|e| anyhow::anyhow!("{:?}", e))?;
    let our_out_r = out_r.name().map_err(|e| anyhow::anyhow!("{:?}", e))?;

    let (out_cb, in_cb) = make_cbs(&cfg);

    let stats = Arc::new(SharedStats::default());
    let handler = DuplexHandler {
        in_l,
        in_r,
        out_l,
        out_r,
        out_cb,
        in_cb,
        out_scratch: vec![0.0; MAX_JACK_FRAMES * 2],
        in_scratch: vec![0.0; MAX_JACK_FRAMES * 2],
        rt_tried: false,
        rt_sample_rate: sr as u32,
        rt_handle: None,
        meter: LoadMeter::new(stats.clone()),
    };

    let active = client
        .activate_async(
            XrunNotifier {
                stats: stats.clone(),
            },
            handler,
        )
        .map_err(|e| anyhow::anyhow!("JACK activate: {:?}", e))?;

    try_autoconnect_output(active.as_client(), &our_out_l, &our_out_r, hint.output_device.as_deref());
    try_autoconnect_input(active.as_client(), &our_in_l, &our_in_r, hint.input_device.as_deref());

    let logger = spawn_stats_logger(stats.clone(), sr as u32, buffer_frames, "duplex");
    let bundle = JackStreamBundle {
        _client: active,
        stats,
        logger: Some(logger),
    };
    Ok((AudioStream::new(bundle), cfg))
}

#[cfg(test)]
mod tests {
    /// Round-trip the interleave / de-interleave helper shape used in the
    /// process handlers. Doesn't require a JACK server.
    #[test]
    fn interleave_roundtrip() {
        let frames = 64;
        let mut interleaved = vec![0.0_f32; frames * 2];
        for i in 0..frames {
            interleaved[i * 2] = i as f32;
            interleaved[i * 2 + 1] = -(i as f32);
        }

        let mut l = vec![0.0_f32; frames];
        let mut r = vec![0.0_f32; frames];
        for i in 0..frames {
            l[i] = interleaved[i * 2];
            r[i] = interleaved[i * 2 + 1];
        }

        let mut back = vec![0.0_f32; frames * 2];
        for i in 0..frames {
            back[i * 2] = l[i];
            back[i * 2 + 1] = r[i];
        }
        assert_eq!(interleaved, back);
    }
}

/// The clients owning physical ports, as a picker should offer them.
///
/// `None` when no server answers, which is how the caller knows to ask cpal
/// instead. The probe client is opened and dropped; it never appears in a
/// patchbay for longer than the enumeration takes.
pub(super) fn list_devices() -> Option<super::DeviceList> {
    let (client, _) = Client::new("phonix-devices", ClientOptions::NO_START_SERVER).ok()?;
    let groups = |flags: jack::PortFlags| -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        client
            .ports(None, None, flags | jack::PortFlags::IS_PHYSICAL)
            .into_iter()
            .filter_map(|p| p.split_once(':').map(|(c, _)| c.to_string()))
            .filter(|c| seen.insert(c.clone()))
            .collect()
    };
    Some(super::DeviceList {
        backend: "jack",
        outputs: groups(jack::PortFlags::IS_INPUT),
        inputs: groups(jack::PortFlags::IS_OUTPUT),
    })
}
