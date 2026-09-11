//! Runtime ownership of the audio stream and the MIDI input connections, so
//! both can be torn down and rebuilt WITHOUT restarting the application.
//!
//! Every binary used to open its stream and connect its MIDI ports in `main`,
//! then keep them alive in locals for the process lifetime. That works until
//! the hardware changes: plug in an interface or a controller after startup and
//! nothing notices, because nothing ever looks again. The only cure was to quit
//! and relaunch.
//!
//! NOT `Send`, and that is not an oversight. `AudioStream` wraps a
//! `cpal::Stream`, whose destructor must run on the thread that created it, so
//! the host has to live wherever the reopen is triggered from. In practice that
//! is the GUI thread, which is why this is owned by the app rather than parked
//! in a worker.
//!
//! The caller supplies a callback FACTORY rather than a callback, because a
//! reopen has to build a fresh callback against the SAME engine. Anything the
//! engine needs to keep across a reopen (the engine itself, the meter-state
//! writer) therefore has to be shared, not owned by the callback.

use crate::audio::{self, AudioStream, AudioStreamConfig, OpenHint, OutputCallback};

/// What a user asked the device layer to do.
///
/// Data, not a widget: whoever draws the control decides how to offer it, and
/// this crate never draws. A GUI passes one of these down; the host acts on it
/// between callbacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioRequest {
    /// Close the stream and open it again, picking up a device that appeared
    /// or a setting that changed.
    RestartAudio,
    /// Ask the MIDI layer to enumerate its inputs again.
    RescanMidi,
}

/// What the device layer is doing, for whoever wants to say so.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostStatus {
    /// Device the stream is currently on, empty when stopped.
    pub device: String,
    pub backend: String,
    /// Number of connected MIDI inputs.
    pub midi_inputs: usize,
    /// Set when the last restart failed, so a GUI can say WHY the device did
    /// not come back rather than looking merely idle.
    pub error: Option<String>,
}

/// A live audio stream plus the MIDI connections feeding it.
pub struct AudioMidiHost<M> {
    stream: Option<AudioStream>,
    config: Option<AudioStreamConfig>,
    hint: OpenHint,
    midi: Vec<M>,
    /// Last failure, kept so the GUI can show WHY the device did not come back
    /// instead of silently staying quiet.
    last_error: Option<String>,
    /// Bumped on every successful (re)open, so a GUI can notice a change
    /// without polling device names.
    epoch: u64,
}

impl<M> AudioMidiHost<M> {
    pub fn new(hint: OpenHint) -> Self {
        Self { stream: None, config: None, hint, midi: Vec::new(), last_error: None, epoch: 0 }
    }

    pub fn config(&self) -> Option<&AudioStreamConfig> { self.config.as_ref() }
    pub fn last_error(&self) -> Option<&str> { self.last_error.as_deref() }
    pub fn epoch(&self) -> u64 { self.epoch }
    pub fn is_running(&self) -> bool { self.stream.is_some() }
    pub fn midi_count(&self) -> usize { self.midi.len() }

    /// Snapshot for the GUI.
    pub fn status(&self) -> HostStatus {
        HostStatus {
            device:  self.config.as_ref().map(|c| c.device_name.clone()).unwrap_or_default(),
            backend: self.config.as_ref().map(|c| c.backend_name.to_string()).unwrap_or_default(),
            midi_inputs: self.midi.len(),
            error: self.last_error.clone(),
        }
    }

    /// Adopt a stream opened elsewhere (the startup path in `main`, which has
    /// to build the engine before the host can exist).
    pub fn adopt(&mut self, stream: AudioStream, config: AudioStreamConfig) {
        self.stream = Some(stream);
        self.config = Some(config);
        self.last_error = None;
        self.epoch += 1;
    }

    pub fn adopt_midi(&mut self, conns: Vec<M>) {
        self.midi = conns;
    }

    /// Close and reopen the output stream, picking up whatever hardware is
    /// present NOW.
    ///
    /// The old stream is dropped FIRST and explicitly. Opening a second stream
    /// while the first still holds the device fails on exclusive backends, and
    /// leaving both alive briefly would have two callbacks pulling the same
    /// engine. If the reopen fails the host is left stopped with the error
    /// recorded, rather than pretending nothing happened.
    /// `lock_rate` refuses a device whose sample rate differs from the engine's.
    ///
    /// Most engines here cannot be retuned at runtime: their filter
    /// coefficients, envelope increments and delay lengths were all derived
    /// from the rate at construction. Reopening onto a different rate would
    /// leave everything detuned by that ratio with nothing to show for it, so a
    /// mismatched device is closed again and reported. Pass `None` for an
    /// engine that CAN follow (the sequencer's does).
    pub fn restart_audio<F>(&mut self, lock_rate: Option<f32>, make_cb: F) -> Result<(), String>
    where
        F: FnOnce(&AudioStreamConfig) -> Box<dyn OutputCallback>,
    {
        self.stream = None;
        self.config = None;
        match audio::open_output(self.hint.clone(), make_cb) {
            Ok((stream, cfg)) => {
                if let Some(want) = lock_rate {
                    if (cfg.sample_rate - want).abs() > 0.5 {
                        // Drop it: half-open at the wrong rate is worse than shut.
                        drop(stream);
                        let msg = format!(
                            "{} runs at {} Hz but the engine is fixed at {} Hz \
                             — restart the application to change rate",
                            cfg.device_name, cfg.sample_rate as u32, want as u32);
                        self.last_error = Some(msg.clone());
                        return Err(msg);
                    }
                }
                self.stream = Some(stream);
                self.config = Some(cfg);
                self.last_error = None;
                self.epoch += 1;
                Ok(())
            }
            Err(e) => {
                let msg = e.to_string();
                self.last_error = Some(msg.clone());
                Err(msg)
            }
        }
    }

    /// Duplex twin of [`Self::restart_audio`], for a host that also captures
    /// input (VP-330's vocoder mic).
    ///
    /// Same rules: drop first, refuse a rate change when the engine is fixed.
    /// The factory has to mint a FRESH capture ring each time, because the
    /// producer and consumer halves were moved into the callbacks that are
    /// being torn down.
    pub fn restart_duplex<F>(&mut self, lock_rate: Option<f32>, make_cbs: F) -> Result<(), String>
    where
        F: FnOnce(&AudioStreamConfig) -> (Box<dyn OutputCallback>, Box<dyn crate::audio::InputCallback>),
    {
        self.stream = None;
        self.config = None;
        match audio::open_duplex(self.hint.clone(), make_cbs) {
            Ok((stream, cfg)) => {
                if let Some(want) = lock_rate {
                    if (cfg.sample_rate - want).abs() > 0.5 {
                        drop(stream);
                        let msg = format!(
                            "{} runs at {} Hz but the engine is fixed at {} Hz \
                             — restart the application to change rate",
                            cfg.device_name, cfg.sample_rate as u32, want as u32);
                        self.last_error = Some(msg.clone());
                        return Err(msg);
                    }
                }
                self.stream = Some(stream);
                self.config = Some(cfg);
                self.last_error = None;
                self.epoch += 1;
                Ok(())
            }
            Err(e) => {
                let msg = e.to_string();
                self.last_error = Some(msg.clone());
                Err(msg)
            }
        }
    }

    /// Drop every MIDI connection and rebuild from a fresh port scan.
    ///
    /// Dropping first matters as much as it does for audio: ALSA hands a port
    /// to one client at a time, so reconnecting before releasing would silently
    /// skip the very device the user just plugged in.
    pub fn rescan_midi<F>(&mut self, connect: F) -> usize
    where
        F: FnOnce() -> Vec<M>,
    {
        self.midi.clear();
        self.midi = connect();
        self.midi.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint() -> OpenHint {
        OpenHint {
            buffer_ms: 10.0,
            prefer_backend: None,
            app_name: "test",
            output_device: None,
            input_device: None,
        }
    }

    #[test]
    fn a_fresh_host_is_stopped_and_quiet() {
        let h: AudioMidiHost<()> = AudioMidiHost::new(hint());
        assert!(!h.is_running());
        assert!(h.config().is_none());
        assert!(h.last_error().is_none());
        assert_eq!(h.epoch(), 0);
    }

    /// A rescan must RELEASE the old connections before taking new ones.
    ///
    /// Not a style point: ALSA gives a port to one client at a time, so
    /// connecting first would fail on exactly the device the user is trying to
    /// pick up. The drop order is asserted through a witness because nothing
    /// else would catch a reordering.
    #[test]
    fn rescanning_midi_drops_the_old_connections_first() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        struct Conn(Arc<AtomicUsize>);
        impl Drop for Conn {
            fn drop(&mut self) { self.0.fetch_add(1, Ordering::SeqCst); }
        }

        let dropped = Arc::new(AtomicUsize::new(0));
        let mut h = AudioMidiHost::new(hint());
        h.adopt_midi(vec![Conn(dropped.clone()), Conn(dropped.clone())]);
        assert_eq!(h.midi_count(), 2);

        let seen_at_connect = Arc::new(AtomicUsize::new(usize::MAX));
        let seen = seen_at_connect.clone();
        let d2 = dropped.clone();
        let n = h.rescan_midi(move || {
            // By the time we are asked for new connections, the old ones must
            // already be gone.
            seen.store(d2.load(Ordering::SeqCst), Ordering::SeqCst);
            vec![Conn(d2.clone())]
        });

        assert_eq!(n, 1);
        assert_eq!(seen_at_connect.load(Ordering::SeqCst), 2,
            "the old MIDI connections were still alive while reconnecting");
    }

    #[test]
    fn adopting_a_stream_advances_the_epoch() {
        let mut h: AudioMidiHost<()> = AudioMidiHost::new(hint());
        let before = h.epoch();
        h.adopt(
            AudioStream::new(()),
            AudioStreamConfig {
                sample_rate: 48_000.0, channels: 2, buffer_frames: 256,
                backend_name: "test", device_name: "dummy".into(),
            },
        );
        assert!(h.is_running());
        assert_eq!(h.epoch(), before + 1);
        assert_eq!(h.config().map(|c| c.sample_rate), Some(48_000.0));
    }
}
// `HostShell` was here: an adapter that wrapped an `eframe::App` so a binary
// kept ownership of its audio host while egui drove the frame. It belongs to
// whoever draws, not to this crate -- nothing here knows what a window is.
