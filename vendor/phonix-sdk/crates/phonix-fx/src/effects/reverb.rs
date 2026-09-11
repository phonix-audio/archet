//! Eleven rooms from one network: eight combs and four allpasses per
//! channel, four long combs and four diffusing allpasses for the big spaces,
//! early reflections, pre-delay, and per-type shaping (shimmer's pitch
//! shift, the gate, the reverse grains, the infinite hold).

use crate::effect::{Effect, Ports, Value};
use super::lines::{AllPassFilter, CombFilter};
use super::math::*;
use crate::spec::{Category, Curve, EffectSpec, Needs, ParamFlags, ParamKind, ParamSpec, Unit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReverbType {
    Hall,
    Room,
    Plate,
    Spring,
    Shimmer,
    Cathedral,
    Cave,
    Ambient,
    Gated,
    Reverse,
    Infinite,
}

const TYPES: [&str; 11] = ["hall", "room", "plate", "spring", "shimmer", "cathedral", "cave", "ambient", "gated", "reverse", "infinite"];
const TYPE_LABELS: [&str; 11] = ["Hall", "Room", "Plate", "Spring", "Shimmer", "Cathedral", "Cave", "Ambient", "Gated", "Reverse", "Infinite"];

impl ReverbType {
    pub const ALL: [ReverbType; 11] = [
        Self::Hall, Self::Room, Self::Plate, Self::Spring, Self::Shimmer, Self::Cathedral,
        Self::Cave, Self::Ambient, Self::Gated, Self::Reverse, Self::Infinite,
    ];

    pub fn named(name: &str) -> Self {
        TYPES.iter().position(|t| *t == name).map(|i| Self::ALL[i]).unwrap_or(Self::Hall)
    }

    pub fn name(self) -> &'static str {
        TYPES[Self::ALL.iter().position(|t| *t == self).unwrap_or(0)]
    }
}

pub struct Reverb {
    sample_rate: f32,
    /// `1.0 / sample_rate` cache for per-sample LFO phase advances
    /// in the Shimmer / Modulation paths (was 3+ FP-divisions per
    /// audio sample under those reverb types).
    inv_sample_rate: f32,

    // Reverb type
    reverb_type: ReverbType,

    // Parameters
    size: f32,      // Room size (0-1)
    decay: f32,     // Decay time (0-1, maps to different ranges per type)
    damping: f32,   // High frequency damping
    pre_delay: f32, // Pre-delay time in seconds (up to 500ms)
    width: f32,     // Stereo width

    // Freeverb-style comb filters (8 per channel)
    comb_l: [CombFilter; 8],
    comb_r: [CombFilter; 8],

    // Freeverb-style allpass filters (4 per channel)
    allpass_l: [AllPassFilter; 4],
    allpass_r: [AllPassFilter; 4],

    // Extra long comb filters for Cathedral/Cave/Infinite (4 per channel)
    long_comb_l: [CombFilter; 4],
    long_comb_r: [CombFilter; 4],

    // Extra allpass for diffusion in large spaces (4 per channel)
    diff_allpass_l: [AllPassFilter; 4],
    diff_allpass_r: [AllPassFilter; 4],

    // Pre-delay buffer (stereo interleaved, up to 500ms)
    pre_delay_buffer: Vec<f32>,
    pre_delay_pos: usize,

    // Reverse reverb buffer (stereo interleaved, up to 2 seconds)
    reverse_buffer: Vec<f32>,
    reverse_write_pos: usize,
    reverse_read_pos: f32,
    reverse_grain_phase: f32,

    // Shimmer
    shimmer_phase: f32,
    shimmer_phase2: f32,

    // Early reflections tapped delay line (mono in, stereo out)
    er_buffer: Vec<f32>,
    er_write_pos: usize,

    // Gated reverb
    gate_env: f32,
    gate_hold_counter: usize,
    // Cached gated hold-time in samples — derives only from `size` + sample_rate,
    // refreshed in set_size() to avoid the per-sample cast in process_gated().
    cached_gate_hold_samples: usize,

    // Modulation LFO for ambient/shimmer
    mod_phase: f32,

    // Fade-in counter after type switch — ramps input from 0→1 over ~50ms
    // to prevent transient blowup when switching from high-feedback types
    type_switch_fade: u32,
}

impl Reverb {
    pub fn new(sample_rate: f32) -> Self {
        let scale = sample_rate / 44100.0;

        // Standard Freeverb tunings
        const COMB_TUNING_L: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        const COMB_TUNING_R: [usize; 8] = [1139, 1211, 1300, 1379, 1445, 1514, 1580, 1640];
        const ALLPASS_TUNING_L: [usize; 4] = [556, 441, 341, 225];
        const ALLPASS_TUNING_R: [usize; 4] = [579, 464, 364, 248];

        // Long comb tunings for large spaces (Cathedral, Cave, Infinite)
        // These are 3-5x longer than standard for massive tail
        const LONG_COMB_L: [usize; 4] = [4799, 5399, 5903, 6421];
        const LONG_COMB_R: [usize; 4] = [4919, 5501, 6007, 6553];

        // Extra diffusion allpass tunings
        const DIFF_ALLPASS_L: [usize; 4] = [1024, 743, 512, 379];
        const DIFF_ALLPASS_R: [usize; 4] = [1051, 769, 537, 397];

        let comb_l = COMB_TUNING_L.map(|t| CombFilter::new((t as f32 * scale) as usize));
        let comb_r = COMB_TUNING_R.map(|t| CombFilter::new((t as f32 * scale) as usize));
        let allpass_l = ALLPASS_TUNING_L.map(|t| AllPassFilter::new((t as f32 * scale) as usize));
        let allpass_r = ALLPASS_TUNING_R.map(|t| AllPassFilter::new((t as f32 * scale) as usize));
        let long_comb_l = LONG_COMB_L.map(|t| CombFilter::new((t as f32 * scale) as usize));
        let long_comb_r = LONG_COMB_R.map(|t| CombFilter::new((t as f32 * scale) as usize));
        let diff_allpass_l =
            DIFF_ALLPASS_L.map(|t| AllPassFilter::new((t as f32 * scale) as usize));
        let diff_allpass_r =
            DIFF_ALLPASS_R.map(|t| AllPassFilter::new((t as f32 * scale) as usize));

        // Pre-delay: up to 500ms stereo interleaved
        let pre_delay_samples = (sample_rate * 0.5) as usize;
        // Reverse buffer: up to 2 seconds stereo interleaved
        let reverse_samples = (sample_rate * 2.0) as usize;
        // Early reflections buffer: up to 100ms mono
        let er_samples = (sample_rate * 0.1) as usize;

        let mut s = Self {
            sample_rate,
            inv_sample_rate: 1.0 / sample_rate.max(1.0),
            reverb_type: ReverbType::Hall,
            size: 0.7,
            decay: 0.5,
            damping: 0.5,
            pre_delay: 0.0,
            width: 1.0,
            comb_l,
            comb_r,
            allpass_l,
            allpass_r,
            long_comb_l,
            long_comb_r,
            diff_allpass_l,
            diff_allpass_r,
            pre_delay_buffer: vec![0.0; pre_delay_samples * 2],
            pre_delay_pos: 0,
            er_buffer: vec![0.0; er_samples.max(1)],
            er_write_pos: 0,
            reverse_buffer: vec![0.0; reverse_samples * 2],
            reverse_write_pos: 0,
            reverse_read_pos: 0.0,
            reverse_grain_phase: 0.0,
            shimmer_phase: 0.0,
            shimmer_phase2: 0.0,
            gate_env: 0.0,
            gate_hold_counter: 0,
            cached_gate_hold_samples: 0,
            mod_phase: 0.0,
            type_switch_fade: 0,
        };
        Self::refresh_static(&mut s);
        s
    }

    /// Refresh derived constants that depend on `size` / sample_rate.
    /// Cheap — just a multiply + cast. Called from setters that touch `size`.
    fn refresh_static(s: &mut Self) {
        s.cached_gate_hold_samples = ((s.size * 0.75 + 0.05) * s.sample_rate) as usize;
    }

    /// Process pre-delay (up to 500ms)
    fn process_pre_delay(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        if self.pre_delay <= 0.0001 {
            return (input_l, input_r);
        }

        let buffer_len = self.pre_delay_buffer.len() / 2;
        if buffer_len < 2 {
            return (input_l, input_r);
        }

        let delay_samples = ((self.pre_delay * self.sample_rate) as usize).min(buffer_len - 1);

        self.pre_delay_buffer[self.pre_delay_pos * 2] = input_l;
        self.pre_delay_buffer[self.pre_delay_pos * 2 + 1] = input_r;

        let read_pos = (self.pre_delay_pos + buffer_len - delay_samples) % buffer_len;
        let delayed_l = self.pre_delay_buffer[read_pos * 2];
        let delayed_r = self.pre_delay_buffer[read_pos * 2 + 1];

        self.pre_delay_pos = (self.pre_delay_pos + 1) % buffer_len;

        (delayed_l, delayed_r)
    }

    /// Early reflections via tapped delay line.
    /// Returns (er_l, er_r, feed) where feed is the ER sum to feed into late reverb.
    fn process_early_reflections(&mut self, input: f32) -> (f32, f32, f32) {
        let buf_len = self.er_buffer.len();
        if buf_len < 2 {
            return (0.0, 0.0, input);
        }

        // Write input into circular buffer. Bounded advance is
        // cheaper than `% buf_len` (non-power-of-2, would compile
        // to an integer divide per audio sample).
        self.er_buffer[self.er_write_pos] = input;
        self.er_write_pos += 1;
        if self.er_write_pos >= buf_len { self.er_write_pos = 0; }

        // Tap times in samples at 44.1kHz — scaled by sample rate and room size.
        // These model first-order reflections from walls, ceiling, floor of a
        // rectangular room. L/R taps differ to create natural stereo from mono input.
        //
        // Tap times (ms at 44.1k): ~5, 8, 12, 17, 21, 27, 33, 39, 47, 55, 67, 80
        // Gains decay with distance (inverse square approximation)
        const TAP_TIMES_L: [usize; 12] = [
            221, 353, 530, 750, 926, 1191, 1456, 1720, 2073, 2425, 2955, 3528,
        ];
        const TAP_TIMES_R: [usize; 12] = [
            243, 397, 573, 794, 1014, 1279, 1544, 1808, 2161, 2513, 3043, 3616,
        ];
        const TAP_GAINS: [f32; 12] = [
            0.85, 0.72, 0.60, 0.50, 0.42, 0.35, 0.29, 0.24, 0.19, 0.15, 0.11, 0.08,
        ];

        let scale = self.sample_rate / 44100.0;
        // Size scales the tap spread: small rooms = close reflections, large = spread
        let size_scale = 0.3 + self.size * 0.7; // 0.3..1.0

        let mut er_l = 0.0f32;
        let mut er_r = 0.0f32;

        for i in 0..12 {
            let tap_l = ((TAP_TIMES_L[i] as f32 * scale * size_scale) as usize).min(buf_len - 1);
            let tap_r = ((TAP_TIMES_R[i] as f32 * scale * size_scale) as usize).min(buf_len - 1);
            let gain = TAP_GAINS[i];

            let read_l = (self.er_write_pos + buf_len - tap_l) % buf_len;
            let read_r = (self.er_write_pos + buf_len - tap_r) % buf_len;

            er_l += self.er_buffer[read_l] * gain;
            er_r += self.er_buffer[read_r] * gain;
        }

        // Normalize so total ER energy ≈ 1x input (sum of gains ≈ 4.2, /4.2)
        er_l *= 0.24;
        er_r *= 0.24;

        // Feed to late reverb: mix of input + attenuated ER tail
        let feed = input + (er_l + er_r) * 0.15;

        (er_l, er_r, feed)
    }

    /// Core Freeverb processing through comb + allpass network
    fn process_core(&mut self, input: f32, feedback: f32, damping: f32) -> (f32, f32) {
        let mut out_l = 0.0f32;
        let mut out_r = 0.0f32;

        // Freeverb-style input scaling: combs with high feedback have peak gain
        // of 1/(1-feedback), so scale input to prevent level blowup.
        // Standard Freeverb uses fixedgain=0.015 with wet output gain=3.0;
        // we fold both into a single input gain: 0.015 * 3.0 ≈ 0.045.
        const INPUT_GAIN: f32 = 0.045;
        let scaled_input = input * INPUT_GAIN;

        for i in 0..8 {
            self.comb_l[i].set_feedback(feedback);
            self.comb_l[i].set_damping(damping);
            self.comb_r[i].set_feedback(feedback);
            self.comb_r[i].set_damping(damping);

            out_l += self.comb_l[i].process(scaled_input);
            out_r += self.comb_r[i].process(scaled_input);
        }

        for i in 0..4 {
            out_l = self.allpass_l[i].process(out_l);
            out_r = self.allpass_r[i].process(out_r);
        }

        (out_l, out_r)
    }

    /// Long comb processing for cavernous spaces
    fn process_long_combs(&mut self, input: f32, feedback: f32, damping: f32) -> (f32, f32) {
        let mut out_l = 0.0f32;
        let mut out_r = 0.0f32;

        const INPUT_GAIN: f32 = 0.045;
        let scaled_input = input * INPUT_GAIN;

        for i in 0..4 {
            self.long_comb_l[i].set_feedback(feedback);
            self.long_comb_l[i].set_damping(damping);
            self.long_comb_r[i].set_feedback(feedback);
            self.long_comb_r[i].set_damping(damping);

            out_l += self.long_comb_l[i].process(scaled_input);
            out_r += self.long_comb_r[i].process(scaled_input);
        }

        for i in 0..4 {
            out_l = self.diff_allpass_l[i].process(out_l);
            out_r = self.diff_allpass_r[i].process(out_r);
        }

        (out_l, out_r)
    }

    /// Apply stereo width
    fn apply_width(&self, out_l: f32, out_r: f32) -> (f32, f32) {
        let mid = (out_l + out_r) * 0.5;
        let side = (out_l - out_r) * 0.5 * self.width;
        (mid + side, mid - side)
    }

    /// Process stereo sample
    fn tick(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        let (pre_l, pre_r) = self.process_pre_delay(input_l, input_r);
        let mut input_mono = (pre_l + pre_r) * 0.5;

        // After a type switch, fade input in over ~50ms to prevent transient spikes
        // from sudden full-level signal hitting fresh high-feedback filters.
        if self.type_switch_fade > 0 {
            let fade_total = (self.sample_rate * 0.05) as f32;
            let fade_gain = 1.0 - (self.type_switch_fade as f32 / fade_total);
            input_mono *= fade_gain;
            self.type_switch_fade -= 1;
        }

        // Early reflections: discrete echoes that give the ear room geometry cues.
        // Some types (Plate, Spring, Reverse, Infinite) skip ER for character reasons.
        let (er_l, er_r, late_input) = match self.reverb_type {
            ReverbType::Plate | ReverbType::Spring | ReverbType::Reverse | ReverbType::Infinite => {
                (0.0, 0.0, input_mono)
            }
            _ => self.process_early_reflections(input_mono),
        };

        // Late reverb (diffuse tail) — fed by ER output for smooth ER→tail crossover
        let (mut wet_l, mut wet_r) = match self.reverb_type {
            ReverbType::Hall => self.process_hall(late_input),
            ReverbType::Room => self.process_room(late_input),
            ReverbType::Plate => self.process_plate(input_mono),
            ReverbType::Spring => self.process_spring(input_mono),
            ReverbType::Shimmer => self.process_shimmer(late_input),
            ReverbType::Cathedral => self.process_cathedral(late_input),
            ReverbType::Cave => self.process_cave(late_input),
            ReverbType::Ambient => self.process_ambient(late_input, pre_l, pre_r),
            ReverbType::Gated => self.process_gated(late_input, input_l, input_r),
            ReverbType::Reverse => self.process_reverse(input_mono),
            ReverbType::Infinite => self.process_infinite(input_mono),
        };

        // Blend early reflections with late tail
        wet_l += er_l;
        wet_r += er_r;

        let (wet_l, wet_r) = self.apply_width(wet_l, wet_r);

        // Guard against NaN/inf from filter blowup
        let wet_l = if wet_l.is_finite() {
            wet_l.clamp(-4.0, 4.0)
        } else {
            0.0
        };
        let wet_r = if wet_r.is_finite() {
            wet_r.clamp(-4.0, 4.0)
        } else {
            0.0
        };

        (wet_l, wet_r)
    }

    // --- Type-specific algorithms ---

    /// Hall: classic large concert hall, warm and enveloping
    fn process_hall(&mut self, input: f32) -> (f32, f32) {
        let feedback = self.decay * 0.3 + 0.6; // 0.6-0.9: long natural tail
        let damping = self.damping * 0.6 + 0.1;
        let (mut out_l, mut out_r) = self.process_core(input, feedback, damping);
        let scale = self.size * 0.8 + 0.6;
        out_l *= scale;
        out_r *= scale;
        (out_l, out_r)
    }

    /// Room: small, tight reflections with quick decay
    fn process_room(&mut self, input: f32) -> (f32, f32) {
        let feedback = self.decay * 0.25 + 0.35; // 0.35-0.6: short tail
        let damping = self.damping * 0.4 + 0.4; // more damped = tighter
        let (mut out_l, mut out_r) = self.process_core(input, feedback, damping);
        let scale = self.size * 0.5 + 0.8;
        out_l *= scale;
        out_r *= scale;
        (out_l, out_r)
    }

    /// Plate: bright, dense, metallic character
    fn process_plate(&mut self, input: f32) -> (f32, f32) {
        let feedback = self.decay * 0.3 + 0.55; // 0.55-0.85: medium-long
        let damping = self.damping * 0.3 + 0.05; // low damping = bright
        let (mut out_l, mut out_r) = self.process_core(input, feedback, damping);
        let scale = self.size * 0.7 + 0.8;
        out_l *= scale;
        out_r *= scale;
        (out_l, out_r)
    }

    /// Spring: bouncy, metallic, vintage character
    fn process_spring(&mut self, input: f32) -> (f32, f32) {
        let feedback = self.decay * 0.2 + 0.4; // 0.4-0.6: short bouncy
        let damping = self.damping * 0.5 + 0.2;
        let (mut out_l, mut out_r) = self.process_core(input, feedback, damping);
        // Spring character: emphasize the "drip" by adding some of the early comb output
        let drip = self.comb_l[0].process(input * 0.1) * 0.15;
        out_l += drip;
        out_r += drip * 0.7;
        let scale = self.size * 0.4 + 0.7;
        out_l *= scale;
        out_r *= scale;
        (out_l, out_r)
    }

    /// Shimmer: ethereal, pitch-shifted reverb with octave harmonics
    fn process_shimmer(&mut self, input: f32) -> (f32, f32) {
        let feedback = self.decay * 0.3 + 0.6; // 0.6-0.9
        let damping = self.damping * 0.3 + 0.05; // bright
        let (mut out_l, mut out_r) = self.process_core(input, feedback, damping);

        // Dual shimmer LFOs at different rates for richer effect
        self.shimmer_phase += 1.5 * self.inv_sample_rate;
        if self.shimmer_phase > 1.0 {
            self.shimmer_phase -= 1.0;
        }
        self.shimmer_phase2 += 2.3 * self.inv_sample_rate;
        if self.shimmer_phase2 > 1.0 {
            self.shimmer_phase2 -= 1.0;
        }

        let shimmer1 = fast_sin(self.shimmer_phase * std::f32::consts::TAU);
        let shimmer2 = fast_sin(self.shimmer_phase2 * std::f32::consts::TAU);

        let shimmer_amount = self.size * 0.15;
        out_l *= 1.0 + shimmer1 * shimmer_amount;
        out_r *= 1.0 + shimmer2 * shimmer_amount;

        // Add octave-up harmonics through the long combs
        let (long_l, long_r) = self.process_long_combs(input * 0.3, feedback * 0.85, damping);
        out_l += long_l * 0.2 * self.size;
        out_r += long_r * 0.2 * self.size;

        let scale = self.size * 0.7 + 0.7;
        out_l *= scale;
        out_r *= scale;
        (out_l, out_r)
    }

    /// Cathedral: massive space with very long, diffuse tail (5-20+ seconds)
    fn process_cathedral(&mut self, input: f32) -> (f32, f32) {
        // High feedback for enormous tail, but capped at 0.95 to stay stable
        let feedback = self.decay * 0.1 + 0.84; // 0.84-0.94
        let damping = self.damping * 0.5 + 0.15;

        // Standard network
        let (mut out_l, mut out_r) = self.process_core(input, feedback, damping);

        // Long combs for the massive tail (slightly higher feedback than core)
        let long_feedback = (feedback * 1.02).min(0.96);
        let (long_l, long_r) = self.process_long_combs(input, long_feedback, damping * 0.7);

        // Blend standard + long for layered reflections
        out_l = out_l * 0.4 + long_l * 0.6;
        out_r = out_r * 0.4 + long_r * 0.6;

        let scale = self.size * 0.5 + 0.7;
        out_l *= scale;
        out_r *= scale;
        (out_l, out_r)
    }

    /// Cave: dark, cavernous with heavy low-end and long pre-delay feel
    fn process_cave(&mut self, input: f32) -> (f32, f32) {
        let feedback = self.decay * 0.12 + 0.80; // 0.80-0.92: long but stable
        let damping = self.damping * 0.3 + 0.55; // heavily damped = dark

        let (mut out_l, mut out_r) = self.process_core(input, feedback, damping);

        // Long combs with heavy damping for dark rumble
        // (process_long_combs already applies diff_allpass internally)
        let long_fb = (feedback * 0.98).min(0.90);
        let (long_l, long_r) = self.process_long_combs(input, long_fb, damping + 0.1);

        out_l = out_l * 0.4 + long_l * 0.6;
        out_r = out_r * 0.4 + long_r * 0.6;

        let scale = self.size * 0.5 + 0.5;
        out_l *= scale;
        out_r *= scale;
        (out_l, out_r)
    }

    /// Ambient: lush, modulated, wide pad-like reverb
    fn process_ambient(&mut self, input: f32, stereo_l: f32, stereo_r: f32) -> (f32, f32) {
        let feedback = self.decay * 0.2 + 0.68; // 0.68-0.88
        let damping = self.damping * 0.4 + 0.1; // relatively bright

        // Modulate the comb feedback slightly for chorus-like lushness
        self.mod_phase += 0.3 * self.inv_sample_rate;
        if self.mod_phase > 1.0 {
            self.mod_phase -= 1.0;
        }
        let modulation = fast_sin(self.mod_phase * std::f32::consts::TAU) * 0.015;

        let (mut out_l, mut out_r) = self.process_core(input, feedback + modulation, damping);

        // Process stereo input separately for wide imaging
        let (long_l, long_r) = self.process_long_combs(
            (stereo_l - stereo_r) * 0.3, // side signal for width
            (feedback * 0.9).min(0.92),
            damping * 0.8,
        );

        out_l += long_l * 0.3;
        out_r -= long_r * 0.3; // inverted for wider image

        let scale = self.size * 0.7 + 0.7;
        out_l *= scale;
        out_r *= scale;
        (out_l, out_r)
    }

    /// Gated: 80s-style reverb with abrupt cutoff after a hold time
    fn process_gated(&mut self, input: f32, raw_l: f32, raw_r: f32) -> (f32, f32) {
        // Detect input level for gate trigger
        let input_level = (raw_l.abs() + raw_r.abs()) * 0.5;
        let gate_threshold = 0.005;

        // Gate hold time based on size parameter (50ms to 800ms) — cached.
        if input_level > gate_threshold {
            self.gate_env = 1.0;
            self.gate_hold_counter = self.cached_gate_hold_samples;
        } else if self.gate_hold_counter > 0 {
            self.gate_hold_counter -= 1;
        } else {
            // Sharp exponential decay when gate closes
            self.gate_env *= 0.995;
            if self.gate_env < 0.001 {
                self.gate_env = 0.0;
            }
        }

        // Dense reverb with medium feedback (the gate does the shaping)
        let feedback = self.decay * 0.2 + 0.65;
        let damping = self.damping * 0.4 + 0.2;
        let (mut out_l, mut out_r) = self.process_core(input, feedback, damping);

        let scale = self.size * 0.7 + 1.0;
        out_l *= scale * self.gate_env;
        out_r *= scale * self.gate_env;
        (out_l, out_r)
    }

    /// Reverse: reversed reverb tail for swelling, pre-verb effects
    fn process_reverse(&mut self, input: f32) -> (f32, f32) {
        let buf_len = self.reverse_buffer.len() / 2;
        if buf_len < 4 {
            return (0.0, 0.0);
        }

        // Write input into forward buffer. Bounded advance is
        // cheaper than `% buf_len` per sample.
        self.reverse_buffer[self.reverse_write_pos * 2] = input;
        self.reverse_buffer[self.reverse_write_pos * 2 + 1] = input;
        self.reverse_write_pos += 1;
        if self.reverse_write_pos >= buf_len { self.reverse_write_pos = 0; }

        // Grain length based on size (100ms to 1.5 seconds)
        let grain_len = ((self.size * 1.4 + 0.1) * self.sample_rate) as usize;
        let grain_len = grain_len.min(buf_len - 1).max(64);

        // Advance grain phase
        self.reverse_grain_phase += 1.0;
        let grain_pos = self.reverse_grain_phase as usize;

        if grain_pos >= grain_len {
            self.reverse_grain_phase = 0.0;
            // Snap read position to current write position
            self.reverse_read_pos = self.reverse_write_pos as f32;
        }

        // Read backwards from the read position
        let progress = self.reverse_grain_phase / grain_len as f32;
        let read_offset = (progress * grain_len as f32) as usize;
        let read_idx = (self.reverse_read_pos as usize + buf_len - read_offset) % buf_len;

        let raw = self.reverse_buffer[read_idx * 2];

        // Window function for smooth crossfade (Hann window)
        let window = 0.5 * (1.0 - (progress * std::f32::consts::TAU).cos());

        let reversed = raw * window;

        // Process the reversed signal through a short reverb tail
        let feedback = self.decay * 0.2 + 0.5;
        let damping = self.damping * 0.4 + 0.2;
        let (mut out_l, mut out_r) = self.process_core(reversed, feedback, damping);

        let scale = 1.5;
        out_l *= scale;
        out_r *= scale;
        (out_l, out_r)
    }

    /// Infinite: freeze/sustain reverb that never decays (drone pad)
    fn process_infinite(&mut self, input: f32) -> (f32, f32) {
        // Near-unity feedback = infinite sustain
        // Decay knob controls how much new input feeds in (freeze amount)
        let feedback = 0.995;
        // Very low input gain to compensate for extreme feedback accumulation
        let input_gain = (1.0 - self.decay * 0.95) * 0.1;
        let damping = self.damping * 0.3 + 0.05; // keep it bright for pads

        let scaled_input = input * input_gain;

        let (mut out_l, mut out_r) = self.process_core(scaled_input, feedback, damping);

        // Long combs also at near-unity for massive cloud
        let (long_l, long_r) = self.process_long_combs(scaled_input, 0.993, damping * 0.5);

        out_l = out_l * 0.5 + long_l * 0.5;
        out_r = out_r * 0.5 + long_r * 0.5;

        // Gentle modulation to prevent static buildup
        self.mod_phase += 0.15 * self.inv_sample_rate;
        if self.mod_phase > 1.0 {
            self.mod_phase -= 1.0;
        }
        let mod_val = fast_sin(self.mod_phase * std::f32::consts::TAU) * 0.01;
        out_l *= 1.0 + mod_val;
        out_r *= 1.0 - mod_val;

        let scale = self.size * 0.6 + 0.8;
        out_l *= scale;
        out_r *= scale;
        (out_l, out_r)
    }

    /// Set reverb type — resets all internal buffers to prevent blowup
    /// from residual energy in comb/allpass filters.
    pub fn set_reverb_type(&mut self, reverb_type: ReverbType) {
        if self.reverb_type != reverb_type {
            self.reverb_type = reverb_type;
            // Critical: flush all filter state so old resonance doesn't
            // feed into the new algorithm's feedback path.
            self.reset();
            // Fade input in over ~50ms to let the new algorithm settle
            // without transient spikes from sudden full-level input.
            self.type_switch_fade = (self.sample_rate * 0.05) as u32;
        }
    }

    pub fn set_size(&mut self, size: f32) {
        self.size = size.max(0.0).min(1.0);
        Self::refresh_static(self);
    }

    pub fn set_decay(&mut self, decay: f32) {
        self.decay = decay.max(0.0).min(1.0);
    }

    pub fn set_damping(&mut self, damping: f32) {
        self.damping = damping.max(0.0).min(1.0);
    }

    /// Set pre-delay time (up to 500ms)
    pub fn set_pre_delay(&mut self, pre_delay: f32) {
        self.pre_delay = pre_delay.max(0.0).min(0.5);
    }

    pub fn set_width(&mut self, width: f32) {
        self.width = width.max(0.0).min(2.0);
    }

    /// Reset reverb
    pub fn reset(&mut self) {
        for comb in self.comb_l.iter_mut().chain(self.comb_r.iter_mut()) {
            comb.reset();
        }
        for comb in self
            .long_comb_l
            .iter_mut()
            .chain(self.long_comb_r.iter_mut())
        {
            comb.reset();
        }
        for allpass in self.allpass_l.iter_mut().chain(self.allpass_r.iter_mut()) {
            allpass.reset();
        }
        for allpass in self
            .diff_allpass_l
            .iter_mut()
            .chain(self.diff_allpass_r.iter_mut())
        {
            allpass.reset();
        }
        for sample in self.pre_delay_buffer.iter_mut() {
            *sample = 0.0;
        }
        for sample in self.er_buffer.iter_mut() {
            *sample = 0.0;
        }
        self.er_write_pos = 0;
        for sample in self.reverse_buffer.iter_mut() {
            *sample = 0.0;
        }
        self.pre_delay_pos = 0;
        self.reverse_write_pos = 0;
        self.reverse_read_pos = 0.0;
        self.reverse_grain_phase = 0.0;
        self.shimmer_phase = 0.0;
        self.shimmer_phase2 = 0.0;
        self.gate_env = 0.0;
        self.gate_hold_counter = 0;
        self.mod_phase = 0.0;
    }
}

pub static PARAMS: [ParamSpec; 6] = [
    ParamSpec { id: "type", name: "Type", short: "Type", kind: ParamKind::Enum { variants: &TYPES, labels: &TYPE_LABELS }, unit: Unit::None, default: Value::E("hall"), flags: ParamFlags::NONE },
    ParamSpec { id: "size", name: "Size", short: "Size", kind: ParamKind::Float { min: 0.0, max: 1.0, curve: Curve::Linear }, unit: Unit::Percent, default: Value::F(0.7), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "decay", name: "Decay", short: "Decay", kind: ParamKind::Float { min: 0.0, max: 1.0, curve: Curve::Linear }, unit: Unit::Percent, default: Value::F(0.5), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "damping", name: "Damping", short: "Damp", kind: ParamKind::Float { min: 0.0, max: 1.0, curve: Curve::Linear }, unit: Unit::Percent, default: Value::F(0.5), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "predelay", name: "Pre-Delay", short: "Pre", kind: ParamKind::Float { min: 0.0, max: 0.5, curve: Curve::Linear }, unit: Unit::Seconds, default: Value::F(0.0), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "width", name: "Width", short: "Width", kind: ParamKind::Float { min: 0.0, max: 2.0, curve: Curve::Linear }, unit: Unit::Percent, default: Value::F(1.0), flags: ParamFlags::AUTOMATABLE },
];

pub static SPEC: EffectSpec = EffectSpec {
    kind: "reverb",
    name: "Reverb",
    category: Category::Reverb,
    params: &PARAMS,
    readouts: &[],
    needs: Needs::NONE,
};

pub fn build(sample_rate: f32) -> Box<dyn Effect> {
    Box::new(Reverb::new(sample_rate))
}

impl Effect for Reverb {
    fn spec(&self) -> &'static EffectSpec {
        &SPEC
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.set_reverb_type(ReverbType::named(value.as_variant().unwrap_or("hall"))),
            1 => self.set_size(value.as_f32()),
            2 => self.set_decay(value.as_f32()),
            3 => self.set_damping(value.as_f32()),
            4 => self.set_pre_delay(value.as_f32()),
            5 => self.set_width(value.as_f32()),
            _ => {}
        }
    }

    fn param(&self, index: usize) -> Value {
        match index {
            0 => Value::E(self.reverb_type.name()),
            1 => Value::F(self.size),
            2 => Value::F(self.decay),
            3 => Value::F(self.damping),
            4 => Value::F(self.pre_delay),
            _ => Value::F(self.width),
        }
    }

    fn process(&mut self, ports: &mut Ports<'_>) {
        for i in 0..ports.frames() {
            let (l, r) = self.tick(ports.audio.l[i], ports.audio.r[i]);
            ports.audio.l[i] = l;
            ports.audio.r[i] = r;
        }
    }

    fn reset(&mut self) {
        Reverb::reset(self)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        *self = Reverb::new(sample_rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_is_sound_and_its_size_is_frozen() {
        assert!(SPEC.problems().is_empty(), "{:?}", SPEC.problems());
        assert_eq!(SPEC.params.len(), 6);
    }

    #[test]
    fn every_type_has_a_tail_and_none_blows_up() {
        for t in TYPES {
            let mut rv = Reverb::new(48_000.0);
            rv.set_param(0, Value::E(t));
            assert_eq!(rv.param(0), Value::E(t));
            // The impulse lands after the fade a type switch applies.
            let mut l = vec![0.0_f32; 24_000];
            let mut r = vec![0.0_f32; 24_000];
            l[4800] = 0.5;
            r[4800] = 0.5;
            let mut ports = Ports { audio: crate::effect::StereoMut { l: &mut l, r: &mut r }, buses: &[], sidechain: None, modulator: None, transport: Default::default(), musical: Default::default(), sample_rate: 48_000.0 };
            rv.process(&mut ports);
            assert!(l.iter().chain(r.iter()).all(|v| v.is_finite()), "{t}");
            let tail: f32 = l[9600..].iter().map(|v| v.abs()).sum();
            assert!(tail > 0.0 || t == "gated" || t == "reverse", "{t} has no tail");
        }
    }
}
