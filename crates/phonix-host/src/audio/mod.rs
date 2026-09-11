//! Cross-platform audio backend abstraction.
//!
//! On Linux we prefer a native JACK client (which works against real `jackd`
//! and against PipeWire's `pipewire-jack` server emulation), falling back to
//! `cpal` when no JACK server is reachable. On other platforms we always use
//! `cpal`. The motivation is that cpal on Linux only speaks ALSA, and even on
//! PipeWire systems that means going through the ALSA compatibility shim —
//! which is exactly what costs us realtime headroom.
//!
//! Engines in this crate all share the same realtime contract: given an
//! interleaved `&mut [f32]` block and a channel count, fill the block. The
//! backends below hide the per-platform plumbing (device enumeration, sample
//! format conversion, planar ↔ interleaved, RT promotion) behind a small
//! callback trait so binaries don't have to care which backend is in use.

use anyhow::Result;

mod cpal_backend;
#[cfg(target_os = "linux")]
mod jack_backend;

/// Output callback invoked from the audio thread. Implementations fill `out`
/// with interleaved samples (`channels` samples per frame). `out.len()` is
/// always a multiple of `channels`.
pub trait OutputCallback: Send + 'static {
    fn process(&mut self, out: &mut [f32], channels: usize);
}

/// Input callback invoked from the audio thread with interleaved samples.
pub trait InputCallback: Send + 'static {
    fn process(&mut self, input: &[f32], channels: usize);
}

/// Stream configuration negotiated with the backend. Handed to the caller's
/// factory closure so it can construct engines at the correct sample rate.
#[derive(Clone, Debug)]
pub struct AudioStreamConfig {
    pub sample_rate: f32,
    pub channels: usize,
    pub buffer_frames: u32,
    pub backend_name: &'static str,
    pub device_name: String,
}

/// Caller-supplied hints used when opening a stream.
#[derive(Clone, Debug)]
pub struct OpenHint {
    /// Target buffer duration in milliseconds. Advisory — JACK inherits its
    /// period size from the server and will ignore this.
    pub buffer_ms: f32,
    /// Pin the backend: `Some("jack")`, `Some("cpal")`, or `None` for auto.
    pub prefer_backend: Option<String>,
    /// Application/client name shown to JACK and used for logging.
    pub app_name: &'static str,
    /// Output device to open, by the name `list_devices` reported. `None`
    /// takes the system default. Under JACK the name is the client owning the
    /// physical playback ports to connect to.
    pub output_device: Option<String>,
    /// Input device, same rules as `output_device`.
    pub input_device: Option<String>,
}

impl OpenHint {
    /// Everything at its default, for a caller with nothing to say.
    ///
    /// The application supplies the rest. This crate reads no configuration
    /// file: where a host keeps its settings is the host's business, and a
    /// library that goes looking for one can only guess wrong.
    pub fn new(app_name: &'static str) -> Self {
        Self {
            // Long enough that a first run is quiet on a machine nobody has
            // tuned, short enough to play on. The caller lowers it.
            buffer_ms: 12.0,
            prefer_backend: None,
            app_name,
            output_device: None,
            input_device: None,
        }
    }
}

/// The devices a picker can offer, for the backend that is actually reachable.
///
/// Names are what `OpenHint::output_device` and `input_device` expect back.
#[derive(Clone, Debug, Default)]
pub struct DeviceList {
    /// Which backend produced these names, so a caller can label them.
    pub backend: &'static str,
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
}

/// Enumerate the devices the given backend preference would open.
///
/// Enumeration talks to the sound server and is far too slow for a per-frame
/// call; a caller that draws a menu caches the result and refreshes on demand.
pub fn list_devices(prefer: Option<&str>) -> DeviceList {
    #[cfg(target_os = "linux")]
    {
        if prefer != Some("cpal") {
            if let Some(l) = jack_backend::list_devices() {
                return l;
            }
        }
    }
    let _ = prefer;
    cpal_backend::list_devices()
}

/// Opaque handle to a running audio stream. Dropping it stops the stream.
///
/// Not `Send` because `cpal::Stream` intentionally isn't `Send` on Linux —
/// its destructor has to run on the thread that built it.
pub struct AudioStream {
    _inner: Box<dyn std::any::Any>,
}

impl AudioStream {
    pub(crate) fn new<T: 'static>(inner: T) -> Self {
        Self {
            _inner: Box::new(inner),
        }
    }
}

/// Open an output-only stream.
///
/// `make_cb` is called once with the negotiated stream configuration so the
/// caller can build its engine at the right sample rate, then hand back the
/// boxed callback.
pub fn open_output<F>(
    hint: OpenHint,
    make_cb: F,
) -> Result<(AudioStream, AudioStreamConfig)>
where
    F: FnOnce(&AudioStreamConfig) -> Box<dyn OutputCallback>,
{
    let prefer = hint.prefer_backend.as_deref();

    #[cfg(target_os = "linux")]
    {
        if prefer != Some("cpal") {
            match jack_backend::probe_client(&hint) {
                Ok(client) => {
                    let ret = jack_backend::finish_open_output(client, &hint, make_cb)?;
                    log_selected(&ret.1);
                    return Ok(ret);
                }
                Err(e) => {
                    if prefer == Some("jack") {
                        return Err(anyhow::anyhow!("JACK server unavailable: {}", e));
                    }
                    log::warn!(
                        "JACK server unavailable ({}), falling back to cpal",
                        e
                    );
                }
            }
        }
    }

    let ret = cpal_backend::open_output(&hint, make_cb)?;
    log_selected(&ret.1);
    Ok(ret)
}

/// Open a duplex stream (simultaneous input + output). Used by VP-330 for
/// its vocoder mic path.
pub fn open_duplex<F>(
    hint: OpenHint,
    make_cbs: F,
) -> Result<(AudioStream, AudioStreamConfig)>
where
    F: FnOnce(&AudioStreamConfig) -> (Box<dyn OutputCallback>, Box<dyn InputCallback>),
{
    let prefer = hint.prefer_backend.as_deref();

    #[cfg(target_os = "linux")]
    {
        if prefer != Some("cpal") {
            match jack_backend::probe_client(&hint) {
                Ok(client) => {
                    let ret = jack_backend::finish_open_duplex(client, &hint, make_cbs)?;
                    log_selected(&ret.1);
                    return Ok(ret);
                }
                Err(e) => {
                    if prefer == Some("jack") {
                        return Err(anyhow::anyhow!("JACK server unavailable: {}", e));
                    }
                    log::warn!(
                        "JACK server unavailable ({}), falling back to cpal",
                        e
                    );
                }
            }
        }
    }

    let ret = cpal_backend::open_duplex(&hint, make_cbs)?;
    log_selected(&ret.1);
    Ok(ret)
}

fn log_selected(cfg: &AudioStreamConfig) {
    log::info!(
        "Audio backend: {} ({}), {:.0} Hz, {} ch, {} frames",
        cfg.backend_name,
        cfg.device_name,
        cfg.sample_rate,
        cfg.channels,
        cfg.buffer_frames,
    );
}
