//! cpal backend — lifted and generalised from the original
//! `src/bin/sequencer.rs` audio wiring. Kept intentionally close to the old
//! code so that the fallback path is as regression-safe as possible.

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::{AudioStream, AudioStreamConfig, InputCallback, OpenHint, OutputCallback};

/// Promote the calling thread to realtime scheduling. Called once from the
/// first audio callback; the returned handle is kept alive by the closure so
/// scheduling stays elevated for the lifetime of the stream.
fn promote_rt(
    buffer_frames: u32,
    sample_rate: u32,
) -> Option<audio_thread_priority::RtPriorityHandle> {
    match audio_thread_priority::promote_current_thread_to_real_time(buffer_frames, sample_rate) {
        Ok(h) => {
            log::info!(
                "Audio thread promoted to realtime (buffer={} frames, sr={} Hz)",
                buffer_frames,
                sample_rate
            );
            Some(h)
        }
        Err(e) => {
            log::warn!("Could not promote audio thread to realtime: {:?}", e);
            None
        }
    }
}

/// Find an output device whose default config is actually usable. Tries the
/// host default first, then scores enumerated devices so we prefer routes
/// that are likely connected to speakers (sound-server pseudo-devices first,
/// then analog `hw:`, then HDMI/DP last).
/// The device named `want`, if it is present and usable.
///
/// A name that has gone (interface unplugged, profile switched) yields `None`
/// so the caller falls back to the default: a stale name in the configuration
/// must never be the reason the application comes up silent.
fn output_device_named(
    host: &cpal::Host,
    want: &str,
) -> Option<(cpal::Device, cpal::SupportedStreamConfig)> {
    let mut devices = host.output_devices().ok()?;
    let dev = devices.find(|d| d.name().map(|n| n == want).unwrap_or(false))?;
    match dev.default_output_config() {
        Ok(cfg) => Some((dev, cfg)),
        Err(e) => {
            log::warn!("Requested output '{}' is unusable: {}", want, e);
            None
        }
    }
}

fn find_usable_output(
    host: &cpal::Host,
    want: Option<&str>,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    if let Some(name) = want {
        if let Some(found) = output_device_named(host, name) {
            return Ok(found);
        }
        log::warn!("Output '{}' not found; falling back to the default", name);
    }
    if let Some(dev) = host.default_output_device() {
        match dev.default_output_config() {
            Ok(cfg) => return Ok((dev, cfg)),
            Err(e) => log::warn!(
                "Default output '{}' unusable: {} — falling back to device enumeration",
                dev.name().unwrap_or_default(),
                e
            ),
        }
    }

    let devices: Vec<_> = host
        .output_devices()
        .map_err(|e| anyhow::anyhow!("Failed to enumerate output devices: {}", e))?
        .collect();

    fn score(name: &str) -> u32 {
        let n = name.to_ascii_lowercase();
        if n.starts_with("pipewire") {
            0
        } else if n.starts_with("pulse") {
            1
        } else if n.starts_with("sysdefault") {
            2
        } else if n.starts_with("default") {
            3
        } else if n.contains("hdmi") || n.contains("dp=") || n.contains("iec958") {
            100
        } else if n.starts_with("hw:") || n.starts_with("plughw:") {
            10
        } else {
            50
        }
    }

    let mut scored: Vec<(u32, cpal::Device, String)> = devices
        .into_iter()
        .map(|d| {
            let n = d.name().unwrap_or_default();
            (score(&n), d, n)
        })
        .collect();
    scored.sort_by_key(|(s, _, _)| *s);

    for (_, dev, name) in scored {
        match dev.default_output_config() {
            Ok(cfg) => {
                log::info!("Using fallback output device: {}", name);
                return Ok((dev, cfg));
            }
            Err(e) => log::warn!("Skipping output '{}': {}", name, e),
        }
    }
    Err(anyhow::anyhow!("No usable audio output device found"))
}

/// Negotiate the fixed buffer size: start from the config hint, round to the
/// nearest power of two (ALSA rejects arbitrary values on most hardware),
/// and clamp to the device's advertised range.
/// Whether the device accepts a fixed buffer of `n` frames.
///
/// `SupportedBufferSize::Range` is advertised, not binding: a device can
/// announce a range and then refuse every fixed size inside it. Only an open
/// settles it, so this opens a stream and throws it away.
fn fixed_size_opens(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    n: u32,
) -> bool {
    let mut probe = config.clone();
    probe.buffer_size = cpal::BufferSize::Fixed(n);
    let quiet = |_: cpal::StreamError| {};
    match sample_format {
        cpal::SampleFormat::F32 => device
            .build_output_stream(
                &probe,
                |d: &mut [f32], _: &cpal::OutputCallbackInfo| d.fill(0.0),
                quiet,
                None,
            )
            .is_ok(),
        cpal::SampleFormat::I16 => device
            .build_output_stream(
                &probe,
                |d: &mut [i16], _: &cpal::OutputCallbackInfo| d.fill(0),
                quiet,
                None,
            )
            .is_ok(),
        cpal::SampleFormat::I32 => device
            .build_output_stream(
                &probe,
                |d: &mut [i32], _: &cpal::OutputCallbackInfo| d.fill(0),
                quiet,
                None,
            )
            .is_ok(),
        _ => false,
    }
}

fn negotiate_buffer_size(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    buffer_ms: f32,
) -> (cpal::BufferSize, u32) {
    let sr = config.sample_rate.0 as f32;
    let raw_frames: u32 = ((buffer_ms * 0.001 * sr) as u32).max(64);

    let round_pow2 = |n: u32| -> u32 {
        let lo = n.next_power_of_two() / 2;
        let hi = n.next_power_of_two();
        if lo == 0 {
            hi
        } else if (n as f32 / lo as f32) < (hi as f32 / n as f32) {
            lo
        } else {
            hi
        }
    };
    let target_frames: u32 = round_pow2(raw_frames);

    let mut chosen_frames: Option<u32> = None;
    if let Ok(mut ranges) = device.supported_output_configs() {
        if let Some(range) = ranges.find(|r| {
            r.channels() == config.channels
                && r.sample_format() == sample_format
                && r.min_sample_rate() <= cpal::SampleRate(config.sample_rate.0)
                && r.max_sample_rate() >= cpal::SampleRate(config.sample_rate.0)
        }) {
            if let cpal::SupportedBufferSize::Range { min, max } = range.buffer_size() {
                chosen_frames = Some(target_frames.clamp(*min, *max));
            }
        }
    }

    match chosen_frames {
        Some(n) if fixed_size_opens(device, config, sample_format, n) => {
            log::info!(
                "Requesting fixed audio buffer: {} frames ({:.1} ms at {} Hz)",
                n,
                n as f32 / sr * 1000.0,
                sr as u32
            );
            (cpal::BufferSize::Fixed(n), n)
        }
        Some(n) => {
            log::info!(
                "Device advertises a buffer range but refuses {} frames; \
                 falling back to its default buffer size",
                n
            );
            (cpal::BufferSize::Default, 0)
        }
        None => {
            log::info!("Device does not expose a buffer range; using default size");
            (cpal::BufferSize::Default, 0)
        }
    }
}

/// A stream error reporter that survives a dead device.
///
/// cpal calls the error callback once per failed poll, so a device that has
/// gone away produces thousands of identical lines a second. The first
/// occurrence is reported at once; a repeat of the same message is counted and
/// summarised no more than once per window, and a different message reports
/// immediately.
fn stream_error_logger(what: &'static str) -> impl FnMut(cpal::StreamError) + Send + 'static {
    const WINDOW: std::time::Duration = std::time::Duration::from_secs(5);
    let mut seen: Option<(String, std::time::Instant, u64)> = None;
    move |e| {
        let msg = e.to_string();
        let now = std::time::Instant::now();
        match &mut seen {
            Some((last, since, count)) if *last == msg => {
                *count += 1;
                if now.duration_since(*since) >= WINDOW {
                    log::error!("{}: {} ({} more in {:?})", what, msg, count, now - *since);
                    *since = now;
                    *count = 0;
                }
            }
            _ => {
                log::error!("{}: {}", what, msg);
                seen = Some((msg, now, 0));
            }
        }
    }
}

// `_rt_handle` is captured by the audio callback closure and reassigned
// on the first call (after `promote_current_thread_to_real_time`); we
// hold the new handle in the same binding to keep RT priority promoted
// for the stream's lifetime. The lint can't see RAII-keep-alive, so we
// silence the unused-assignment + unused-capture warnings here.
#[allow(unused_assignments)]
fn build_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    mut cb: Box<dyn OutputCallback>,
    channels: usize,
    rt_buffer_frames: u32,
    rt_sample_rate: u32,
) -> Result<cpal::Stream> {
    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let mut rt_tried = false;
            let mut _rt_handle: Option<audio_thread_priority::RtPriorityHandle> = None;
            device.build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    if !rt_tried {
                        rt_tried = true;
                        _rt_handle = promote_rt(rt_buffer_frames, rt_sample_rate);
                    }
                    // Per-thread, sticky and free; set here because the audio
                    // thread is the backend's, created outside our control.
                    phonix_rt::denormal::enable_flush_to_zero();
                    data.fill(0.0);
                    cb.process(data, channels);
                },
                stream_error_logger("Audio error"),
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let mut buf: Vec<f32> = Vec::new();
            let mut rt_tried = false;
            let mut _rt_handle: Option<audio_thread_priority::RtPriorityHandle> = None;
            device.build_output_stream(
                config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    if !rt_tried {
                        rt_tried = true;
                        _rt_handle = promote_rt(rt_buffer_frames, rt_sample_rate);
                    }
                    // Per-thread, sticky and free; set here because the audio
                    // thread is the backend's, created outside our control.
                    phonix_rt::denormal::enable_flush_to_zero();
                    buf.resize(data.len(), 0.0);
                    buf.fill(0.0);
                    cb.process(&mut buf, channels);
                    for (o, s) in data.iter_mut().zip(buf.iter()) {
                        *o = (*s * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    }
                },
                stream_error_logger("Audio error"),
                None,
            )?
        }
        cpal::SampleFormat::I32 => {
            let mut buf: Vec<f32> = Vec::new();
            let mut rt_tried = false;
            let mut _rt_handle: Option<audio_thread_priority::RtPriorityHandle> = None;
            device.build_output_stream(
                config,
                move |data: &mut [i32], _: &cpal::OutputCallbackInfo| {
                    if !rt_tried {
                        rt_tried = true;
                        _rt_handle = promote_rt(rt_buffer_frames, rt_sample_rate);
                    }
                    // Per-thread, sticky and free; set here because the audio
                    // thread is the backend's, created outside our control.
                    phonix_rt::denormal::enable_flush_to_zero();
                    buf.resize(data.len(), 0.0);
                    buf.fill(0.0);
                    cb.process(&mut buf, channels);
                    for (o, s) in data.iter_mut().zip(buf.iter()) {
                        *o = (*s * 2147483647.0).clamp(-2147483648.0, 2147483647.0) as i32;
                    }
                },
                stream_error_logger("Audio error"),
                None,
            )?
        }
        other => return Err(anyhow::anyhow!("Unsupported sample format: {:?}", other)),
    };
    Ok(stream)
}

fn build_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    mut cb: Box<dyn InputCallback>,
    channels: usize,
) -> Result<cpal::Stream> {
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                cb.process(data, channels);
            },
            stream_error_logger("Audio input error"),
            None,
        )?,
        cpal::SampleFormat::I16 => {
            let mut buf: Vec<f32> = Vec::new();
            device.build_input_stream(
                config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    buf.resize(data.len(), 0.0);
                    for (dst, s) in buf.iter_mut().zip(data.iter()) {
                        *dst = *s as f32 / 32768.0;
                    }
                    cb.process(&buf, channels);
                },
                stream_error_logger("Audio input error"),
                None,
            )?
        }
        cpal::SampleFormat::I32 => {
            let mut buf: Vec<f32> = Vec::new();
            device.build_input_stream(
                config,
                move |data: &[i32], _: &cpal::InputCallbackInfo| {
                    buf.resize(data.len(), 0.0);
                    for (dst, s) in buf.iter_mut().zip(data.iter()) {
                        *dst = *s as f32 / 2147483648.0;
                    }
                    cb.process(&buf, channels);
                },
                stream_error_logger("Audio input error"),
                None,
            )?
        }
        other => return Err(anyhow::anyhow!("Unsupported input sample format: {:?}", other)),
    };
    Ok(stream)
}

pub(super) fn open_output<F>(
    hint: &OpenHint,
    make_cb: F,
) -> Result<(AudioStream, AudioStreamConfig)>
where
    F: FnOnce(&AudioStreamConfig) -> Box<dyn OutputCallback>,
{
    let host = cpal::default_host();
    let (device, supported) = find_usable_output(&host, hint.output_device.as_deref())?;
    let used_name = device.name().unwrap_or_default();
    let sample_format = supported.sample_format();
    let mut config: cpal::StreamConfig = supported.into();
    let sr = config.sample_rate.0 as f32;
    let channels = config.channels as usize;

    let (bufsize, fixed_frames) =
        negotiate_buffer_size(&device, &config, sample_format, hint.buffer_ms);
    config.buffer_size = bufsize;

    let negotiated = AudioStreamConfig {
        sample_rate: sr,
        channels,
        buffer_frames: fixed_frames,
        backend_name: "cpal",
        device_name: used_name,
    };

    let cb = make_cb(&negotiated);

    let stream = build_output_stream(
        &device,
        &config,
        sample_format,
        cb,
        channels,
        fixed_frames,
        config.sample_rate.0,
    )?;
    stream.play()?;
    Ok((AudioStream::new(stream), negotiated))
}

pub(super) fn open_duplex<F>(
    hint: &OpenHint,
    make_cbs: F,
) -> Result<(AudioStream, AudioStreamConfig)>
where
    F: FnOnce(&AudioStreamConfig) -> (Box<dyn OutputCallback>, Box<dyn InputCallback>),
{
    let host = cpal::default_host();
    let (out_device, out_supported) = find_usable_output(&host, hint.output_device.as_deref())?;
    let out_name = out_device.name().unwrap_or_default();
    let out_format = out_supported.sample_format();
    let mut out_config: cpal::StreamConfig = out_supported.into();
    let sr = out_config.sample_rate.0 as f32;
    let out_channels = out_config.channels as usize;

    let (bufsize, fixed_frames) =
        negotiate_buffer_size(&out_device, &out_config, out_format, hint.buffer_ms);
    out_config.buffer_size = bufsize;

    let negotiated = AudioStreamConfig {
        sample_rate: sr,
        channels: out_channels,
        buffer_frames: fixed_frames,
        backend_name: "cpal",
        device_name: out_name,
    };

    let (out_cb, in_cb) = make_cbs(&negotiated);

    // Input: default host input device, keep its native config. We don't
    // couple the input sample rate to the output — the caller's ring buffer
    // absorbs the drift.
    let requested_in = hint.input_device.as_deref().and_then(|want| {
        let found = host
            .input_devices()
            .ok()
            .and_then(|mut it| it.find(|d| d.name().map(|n| n == want).unwrap_or(false)));
        if found.is_none() {
            log::warn!("Input '{}' not found; falling back to the default", want);
        }
        found
    });
    let in_stream = if let Some(in_device) = requested_in.or_else(|| host.default_input_device()) {
        let in_name = in_device.name().unwrap_or_default();
        match in_device.default_input_config() {
            Ok(in_supported) => {
                let in_format = in_supported.sample_format();
                let in_config: cpal::StreamConfig = in_supported.into();
                let in_channels = in_config.channels as usize;
                log::info!(
                    "Mic input: {} ({} Hz, {} ch, {:?})",
                    in_name,
                    in_config.sample_rate.0,
                    in_channels,
                    in_format
                );
                Some(build_input_stream(
                    &in_device,
                    &in_config,
                    in_format,
                    in_cb,
                    in_channels,
                )?)
            }
            Err(e) => {
                log::warn!("No usable input device: {} — running output-only", e);
                None
            }
        }
    } else {
        log::info!("No default input device — running output-only");
        None
    };

    let out_stream = build_output_stream(
        &out_device,
        &out_config,
        out_format,
        out_cb,
        out_channels,
        fixed_frames,
        out_config.sample_rate.0,
    )?;
    out_stream.play()?;
    if let Some(ref s) = in_stream {
        s.play()?;
    }

    // Keep both streams alive together.
    let bundle: (cpal::Stream, Option<cpal::Stream>) = (out_stream, in_stream);
    Ok((AudioStream::new(bundle), negotiated))
}

/// Output and input device names cpal can see on the default host.
pub(super) fn list_devices() -> super::DeviceList {
    let host = cpal::default_host();
    fn dedup(mut v: Vec<String>) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        v.retain(|n| seen.insert(n.clone()));
        v
    }
    let outputs = host
        .output_devices()
        .map(|ds| ds.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    let inputs = host
        .input_devices()
        .map(|ds| ds.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    super::DeviceList {
        backend: "cpal",
        outputs: dedup(outputs),
        inputs: dedup(inputs),
    }
}
