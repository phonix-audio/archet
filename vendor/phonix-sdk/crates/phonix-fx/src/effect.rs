//! The contract every effect implements, and what it is handed per block.
//!
//! One trait, one call per block. Everything an effect might want from its
//! host -- a sidechain, a modulator, the transport, the key -- arrives as
//! data in `Ports`; an effect that ignores a field pays nothing for it, and
//! the host learns from `EffectSpec::needs` what it may skip gathering.

use crate::spec::EffectSpec;

/// A parameter value, typed as the spec declares it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    F(f32),
    I(i32),
    B(bool),
    /// An enum variant, by the name the spec lists.
    E(&'static str),
}

impl Value {
    pub fn as_f32(self) -> f32 {
        match self {
            Value::F(v) => v,
            Value::I(v) => v as f32,
            Value::B(b) => if b { 1.0 } else { 0.0 },
            Value::E(_) => 0.0,
        }
    }
    pub fn as_i32(self) -> i32 {
        match self {
            Value::F(v) => v as i32,
            Value::I(v) => v,
            Value::B(b) => b as i32,
            Value::E(_) => 0,
        }
    }
    pub fn as_bool(self) -> bool {
        match self {
            Value::F(v) => v > 0.5,
            Value::I(v) => v != 0,
            Value::B(b) => b,
            Value::E(_) => false,
        }
    }
    pub fn as_variant(self) -> Option<&'static str> {
        match self {
            Value::E(v) => Some(v),
            _ => None,
        }
    }
}

/// Two channels, read only.
pub struct Stereo<'a> {
    pub l: &'a [f32],
    pub r: &'a [f32],
}

/// Two channels, in place.
pub struct StereoMut<'a> {
    pub l: &'a mut [f32],
    pub r: &'a mut [f32],
}

impl StereoMut<'_> {
    pub fn frames(&self) -> usize {
        self.l.len().min(self.r.len())
    }
}

/// Where the host's clock stands at the start of the block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transport {
    pub bpm: f32,
    /// Beats elapsed at the block's first frame.
    pub beat: f64,
    pub playing: bool,
}

impl Default for Transport {
    fn default() -> Self {
        Transport { bpm: 120.0, beat: 0.0, playing: false }
    }
}

/// The notes the host knows about: a key, and the chord sounding now. Fixed
/// size, so a block never allocates to describe it.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Musical {
    /// The key's root as a MIDI pitch class, when the host has one.
    pub root: Option<u8>,
    /// Scale degrees above the root, in semitones, when the host has one.
    pub scale: Option<&'static [u8]>,
    /// Pitch classes of the chord sounding now.
    pub chord: [u8; 12],
    pub chord_len: u8,
}

impl Musical {
    pub fn chord(&self) -> &[u8] {
        &self.chord[..self.chord_len as usize]
    }
}

/// Everything an effect is handed for one block.
pub struct Ports<'a> {
    pub audio: StereoMut<'a>,
    /// Buses the host filled: a sidechain source, a key input. Which one a
    /// slot listens to is `sidechain`, resolved by the chain from the spec.
    pub buses: &'a [Stereo<'a>],
    pub sidechain: Option<usize>,
    /// The vocoder's carrier, and nothing else's.
    pub modulator: Option<&'a [f32]>,
    pub transport: Transport,
    pub musical: Musical,
    pub sample_rate: f32,
}

impl<'a> Ports<'a> {
    pub fn frames(&self) -> usize {
        self.audio.frames()
    }
    /// The sidechain this slot was given, if the host filled it.
    pub fn sidechain(&self) -> Option<&Stereo<'a>> {
        self.sidechain.and_then(|i| self.buses.get(i))
    }
}

/// One effect.
///
/// Construction is not here: the registry carries `fn(sample_rate) ->
/// Box<dyn Effect>` for each kind. Mix is not here either: an effect runs
/// fully wet and the chain crossfades, so no effect carries a mix of its own.
pub trait Effect: Send {
    fn spec(&self) -> &'static EffectSpec;

    /// By index into `spec().params`. The chain resolves ids to indices once,
    /// at apply time; nothing on the audio thread compares strings.
    fn set_param(&mut self, index: usize, value: Value);
    fn param(&self, index: usize) -> Value;

    /// One block, in place. Any block length.
    fn process(&mut self, ports: &mut Ports<'_>);

    /// Clear delay lines, envelopes and histories; keep parameters.
    fn reset(&mut self);

    /// After this the chain re-applies every parameter of the slot, so an
    /// implementation may discard all of its state; only the tail is lost.
    fn set_sample_rate(&mut self, sample_rate: f32);

    fn latency_samples(&self) -> usize {
        0
    }

    /// A scalar the effect publishes, by index into `spec().readouts`.
    fn readout(&self, _index: usize) -> f32 {
        f32::NAN
    }

    /// An array the effect publishes, by index into `spec().readouts`;
    /// returns how many values were written.
    fn readout_array(&self, _index: usize, _out: &mut [f32]) -> usize {
        0
    }
}
