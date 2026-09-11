//! Eleven delays on one line: digital, ping-pong, tape with wow and flutter,
//! multi-tap, slapback, analog with drift and darkening, reverse grains,
//! ducking, diffused, shimmer, lo-fi. The delayed signal is added to the
//! dry one at `level`; the time follows the transport when synced.

use super::lines::AllPassFilter;
use super::math::*;
use super::tempo::TempoDivision;
use crate::effect::{Effect, Ports, Value};
use crate::spec::{Category, Curve, EffectSpec, Needs, ParamFlags, ParamKind, ParamSpec, Unit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelayType {
    Simple,
    PingPong,
    Tape,
    MultiTap,
    Slapback,
    Analog,
    Reverse,
    Ducking,
    Diffused,
    Shimmer,
    LoFi,
}

const TYPES: [&str; 11] = ["digital", "ping-pong", "tape", "multi-tap", "slapback", "analog", "reverse", "ducking", "diffused", "shimmer", "lo-fi"];
const TYPE_LABELS: [&str; 11] = ["Digital", "Ping Pong", "Tape", "Multi-Tap", "Slapback", "Analog", "Reverse", "Ducking", "Diffused", "Shimmer", "Lo-Fi"];

impl DelayType {
    pub const ALL: [DelayType; 11] = [
        Self::Simple, Self::PingPong, Self::Tape, Self::MultiTap, Self::Slapback, Self::Analog,
        Self::Reverse, Self::Ducking, Self::Diffused, Self::Shimmer, Self::LoFi,
    ];

    pub fn named(name: &str) -> Self {
        TYPES.iter().position(|t| *t == name).map(|i| Self::ALL[i]).unwrap_or(Self::Simple)
    }

    pub fn name(self) -> &'static str {
        TYPES[Self::ALL.iter().position(|t| *t == self).unwrap_or(0)]
    }
}

pub struct Delay {
    sample_rate: f32,
    pi_over_sr: f32,
    /// `1.0 / sample_rate` cache for per-sample LFO/drift phase
    /// advances (`shimmer`, `analog_drift`) — replaces FP divisions
    /// with multiplies on the audio path. Lines 793/924/928 each
    /// fired once per sample under Analog and Shimmer modes.
    inv_sample_rate: f32,

    // Delay type
    delay_type: DelayType,

    // Parameters
    time: f32,          // seconds (when not synced)
    feedback: f32,      // 0.0 to 1.0
    mix: f32,           // 0.0 to 1.0
    stereo_offset: f32, // 0.0 to 1.0 (offset between L/R)

    // Tempo sync
    tempo_sync: bool,
    tempo_division: TempoDivision,
    tempo: f32, // BPM

    // Tape characteristics
    wow_depth: f32,
    wow_rate: f32,
    flutter_depth: f32,
    flutter_rate: f32,
    tape_saturation: f32,

    // Multi-tap settings
    tap_count: usize,

    // High/Low cut for delay feedback
    high_cut: f32,
    low_cut: f32,
    cached_lp_alpha: f32,        // cached 1-pole LP coefficient for high_cut
    cached_hp_alpha: f32,        // cached 1-pole HP coefficient for low_cut
    cached_analog_lp_alpha: f32, // cached analog delay darkening filter (fixed 2500Hz)

    // Delay lines (interleaved stereo: even = L, odd = R)
    delay_buffer: Vec<f32>,
    write_pos: usize,

    // Tape modulation
    lfo_phase_wow: f32,
    lfo_phase_flutter: f32,

    // Filter state for high/low cut
    hp_state: [f32; 2],
    lp_state: [f32; 2],

    // Analog: drift LFO and darkening filter
    analog_drift_phase: f32,
    analog_lp_state: [f32; 2],

    // Reverse: grain-based reverse playback
    reverse_grain_phase: f32,

    // Ducking: envelope follower
    duck_env: f32,

    // Diffused: allpass diffusers (4 per channel)
    diffuse_ap_l: [AllPassFilter; 4],
    diffuse_ap_r: [AllPassFilter; 4],

    // Shimmer: pitch shift phase
    shimmer_phase: f32,
    shimmer_phase2: f32,

    // LoFi: sample rate reduction state
    lofi_hold: [f32; 2],
    lofi_counter: f32,
}

impl Delay {
    pub fn new(sample_rate: f32) -> Self {
        let max_delay_time = 5.0; // 5 seconds max
        let buffer_size = (sample_rate * max_delay_time * 2.0) as usize; // *2 for stereo

        let scale = sample_rate / 44100.0;
        const DIFF_AP_L: [usize; 4] = [601, 433, 311, 197];
        const DIFF_AP_R: [usize; 4] = [619, 449, 331, 211];

        Self {
            sample_rate,
            pi_over_sr: std::f32::consts::PI / sample_rate,
            inv_sample_rate: 1.0 / sample_rate.max(1.0),
            delay_type: DelayType::Simple,
            time: 0.3,
            feedback: 0.4,
            mix: 0.3,
            stereo_offset: 0.0,
            tempo_sync: false,
            tempo_division: TempoDivision::Eighth,
            tempo: 120.0,
            wow_depth: 0.002,
            wow_rate: 0.5,
            flutter_depth: 0.001,
            flutter_rate: 8.0,
            tape_saturation: 0.3,
            tap_count: 4,
            high_cut: 8000.0,
            low_cut: 100.0,
            delay_buffer: vec![0.0; buffer_size],
            write_pos: 0,
            lfo_phase_wow: 0.0,
            lfo_phase_flutter: 0.0,
            hp_state: [0.0; 2],
            lp_state: [0.0; 2],
            analog_drift_phase: 0.0,
            analog_lp_state: [0.0; 2],
            reverse_grain_phase: 0.0,
            duck_env: 0.0,
            diffuse_ap_l: DIFF_AP_L.map(|t| AllPassFilter::new((t as f32 * scale) as usize)),
            diffuse_ap_r: DIFF_AP_R.map(|t| AllPassFilter::new((t as f32 * scale) as usize)),
            shimmer_phase: 0.0,
            shimmer_phase2: 0.0,
            lofi_hold: [0.0; 2],
            lofi_counter: 0.0,
            cached_lp_alpha: 1.0 - (-2.0 * std::f32::consts::PI * 8000.0 / sample_rate).exp(),
            cached_hp_alpha: 1.0 - (-2.0 * std::f32::consts::PI * 100.0 / sample_rate).exp(),
            cached_analog_lp_alpha: 1.0 - (-std::f32::consts::TAU * 2500.0 / sample_rate).exp(),
        }
    }

    fn buffer_size(&self) -> usize {
        self.delay_buffer.len()
    }

    /// Get current delay time in seconds (accounting for tempo sync)
    fn get_delay_time(&self) -> f32 {
        if self.tempo_sync {
            self.tempo_division.to_time_seconds(self.tempo)
        } else {
            self.time
        }
    }

    /// Get tape modulation offset in samples
    fn get_tape_modulation(&mut self) -> f32 {
        if self.delay_type != DelayType::Tape {
            return 0.0;
        }

        // Update LFO phases — fold `rate/sr * 2π` into a single
        // multiply using the cached `pi_over_sr`, removing two FP
        // divisions per audio sample on the tape path.
        let two_pi_over_sr = 2.0 * self.pi_over_sr;
        self.lfo_phase_wow += self.wow_rate * two_pi_over_sr;
        self.lfo_phase_flutter += self.flutter_rate * two_pi_over_sr;

        if self.lfo_phase_wow > 2.0 * std::f32::consts::PI {
            self.lfo_phase_wow -= 2.0 * std::f32::consts::PI;
        }
        if self.lfo_phase_flutter > 2.0 * std::f32::consts::PI {
            self.lfo_phase_flutter -= 2.0 * std::f32::consts::PI;
        }

        // Calculate modulation
        let wow = fast_sin(self.lfo_phase_wow) * self.wow_depth * self.sample_rate;
        let flutter = fast_sin(self.lfo_phase_flutter) * self.flutter_depth * self.sample_rate;

        wow + flutter
    }

    /// Apply tape saturation
    fn apply_tape_saturation(&self, sample: f32) -> f32 {
        if self.delay_type != DelayType::Tape || self.tape_saturation <= 0.0 {
            return sample;
        }

        // Soft saturation curve
        let drive = 1.0 + self.tape_saturation * 3.0;
        let saturated = sample * drive;

        // Tanh-like saturation
        saturated.tanh() / drive.sqrt()
    }

    /// Apply high/low cut filter (correct 1-pole LP and HP) — uses cached coefficients
    #[inline(always)]
    fn apply_filter(&mut self, sample: f32, channel: usize) -> f32 {
        let ch = channel.min(1);

        // Low-pass filter (1-pole): lp += alpha * (input - lp)
        self.lp_state[ch] += self.cached_lp_alpha * (sample - self.lp_state[ch]);

        // High-pass filter: hp = input - lowpass(input)
        self.hp_state[ch] += self.cached_hp_alpha * (self.lp_state[ch] - self.hp_state[ch]);

        self.lp_state[ch] - self.hp_state[ch]
    }

    /// Read from delay buffer with linear interpolation (frame-aligned)
    fn read_delay(&self, channel: usize, delay_samples: f32) -> f32 {
        let num_frames = self.buffer_size() / 2;
        let delay_samples = delay_samples.max(1.0).min((num_frames - 2) as f32);

        let write_frame = self.write_pos / 2;
        let mut read_frame = write_frame as f32 - delay_samples;
        while read_frame < 0.0 {
            read_frame += num_frames as f32;
        }
        let read_frame = read_frame % num_frames as f32;

        let frame_int = read_frame as usize;
        let frac = read_frame - frame_int as f32;
        let frame_next = (frame_int + 1) % num_frames;

        let s1 = self.delay_buffer[frame_int * 2 + channel];
        let s2 = self.delay_buffer[frame_next * 2 + channel];

        s1 + (s2 - s1) * frac
    }

    /// Write to delay buffer (with safety clamp)
    fn write_delay(&mut self, channel: usize, sample: f32) {
        let pos = (self.write_pos + channel) % self.buffer_size();
        self.delay_buffer[pos] = if sample.is_finite() {
            sample.clamp(-4.0, 4.0)
        } else {
            0.0
        };
    }

    /// Process stereo sample
    fn tick(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        if self.mix <= 0.0001 {
            return (input_l, input_r);
        }

        let base_delay_samples = self.get_delay_time() * self.sample_rate;
        let tape_mod = self.get_tape_modulation();

        let result = match self.delay_type {
            DelayType::Simple => {
                let delay_l = base_delay_samples + tape_mod;
                let delay_r = base_delay_samples * (1.0 + self.stereo_offset) + tape_mod;

                let delayed_l = self.read_delay(0, delay_l);
                let delayed_r = self.read_delay(1, delay_r);

                // Apply filtering to feedback
                let fb_l = self.apply_filter(delayed_l, 0);
                let fb_r = self.apply_filter(delayed_r, 1);

                // Apply tape saturation
                let fb_l = self.apply_tape_saturation(fb_l);
                let fb_r = self.apply_tape_saturation(fb_r);

                // Write to buffer with feedback
                self.write_delay(0, input_l + fb_l * self.feedback);
                self.write_delay(1, input_r + fb_r * self.feedback);

                // Output
                let out_l = input_l + delayed_l * self.mix;
                let out_r = input_r + delayed_r * self.mix;

                (out_l, out_r)
            }

            DelayType::PingPong => {
                let delay_samples = base_delay_samples + tape_mod;

                // Read each channel from its own delay line
                let delayed_l = self.read_delay(0, delay_samples);
                let delayed_r = self.read_delay(1, delay_samples);

                let fb_l = self.apply_filter(delayed_l, 0);
                let fb_r = self.apply_filter(delayed_r, 1);
                let fb_l = self.apply_tape_saturation(fb_l);
                let fb_r = self.apply_tape_saturation(fb_r);

                // Mono input sum enters L only; L bounces to R, R bounces to L
                let mono_in = (input_l + input_r) * 0.5;
                self.write_delay(0, mono_in + fb_r * self.feedback);
                self.write_delay(1, fb_l * self.feedback);

                let out_l = input_l + delayed_l * self.mix;
                let out_r = input_r + delayed_r * self.mix;

                (out_l, out_r)
            }

            DelayType::Tape => {
                // Tape delay with wow/flutter and saturation
                let delay_samples = base_delay_samples + tape_mod;

                let delayed_l = self.read_delay(0, delay_samples);
                let delayed_r = self.read_delay(1, delay_samples * (1.0 + self.stereo_offset));

                let fb_l = self.apply_filter(delayed_l, 0);
                let fb_r = self.apply_filter(delayed_r, 1);
                let fb_l = self.apply_tape_saturation(fb_l);
                let fb_r = self.apply_tape_saturation(fb_r);

                self.write_delay(0, input_l + fb_l * self.feedback);
                self.write_delay(1, input_r + fb_r * self.feedback);

                let out_l = input_l + delayed_l * self.mix;
                let out_r = input_r + delayed_r * self.mix;

                (out_l, out_r)
            }

            DelayType::MultiTap => {
                // Multi-tap delay with evenly spaced taps
                let mut out_l = input_l;
                let mut out_r = input_r;

                let taps = self.tap_count.max(2);
                let tap_mix = self.mix / taps as f32;
                let inv_taps = 1.0 / taps as f32;
                for tap in 1..=taps {
                    let tap_delay = base_delay_samples * (tap as f32 * inv_taps);

                    let delayed_l = self.read_delay(0, tap_delay + tape_mod);
                    let delayed_r =
                        self.read_delay(1, tap_delay * (1.0 + self.stereo_offset) + tape_mod);

                    out_l += delayed_l * tap_mix;
                    out_r += delayed_r * tap_mix;
                }

                // Feedback from last tap
                let last_delay = base_delay_samples;
                let fb_l = self.apply_filter(self.read_delay(0, last_delay), 0);
                let fb_r = self.apply_filter(self.read_delay(1, last_delay), 1);

                self.write_delay(0, input_l + fb_l * self.feedback);
                self.write_delay(1, input_r + fb_r * self.feedback);

                (out_l, out_r)
            }

            DelayType::Slapback => {
                // Short delay, single echo, no feedback
                let slap_time = base_delay_samples.min(self.sample_rate * 0.15); // Max 150ms
                let delay_samples = slap_time + tape_mod;

                let delayed_l = self.read_delay(0, delay_samples);
                let delayed_r = self.read_delay(1, delay_samples * (1.0 + self.stereo_offset));

                let out_l = input_l + delayed_l * self.mix;
                let out_r = input_r + delayed_r * self.mix;

                // No feedback for slapback
                self.write_delay(0, input_l);
                self.write_delay(1, input_r);

                (out_l, out_r)
            }

            DelayType::Analog => {
                // Warm analog delay: drift, darkening repeats, subtle saturation
                self.analog_drift_phase += 0.07 * self.inv_sample_rate;
                if self.analog_drift_phase > 1.0 {
                    self.analog_drift_phase -= 1.0;
                }
                let drift = fast_sin(self.analog_drift_phase * std::f32::consts::TAU)
                    * 0.003
                    * self.sample_rate;

                let delay_l = base_delay_samples + drift;
                let delay_r = base_delay_samples * (1.0 + self.stereo_offset) - drift * 0.7;

                let delayed_l = self.read_delay(0, delay_l);
                let delayed_r = self.read_delay(1, delay_r);

                // Progressive darkening: heavy lowpass on feedback (cached 2500Hz coefficient)
                self.analog_lp_state[0] +=
                    self.cached_analog_lp_alpha * (delayed_l - self.analog_lp_state[0]);
                self.analog_lp_state[1] +=
                    self.cached_analog_lp_alpha * (delayed_r - self.analog_lp_state[1]);
                let fb_l = self.analog_lp_state[0];
                let fb_r = self.analog_lp_state[1];

                // Soft saturation on feedback
                let drive = 1.5;
                let fb_l = (fb_l * drive).tanh() / drive.sqrt();
                let fb_r = (fb_r * drive).tanh() / drive.sqrt();

                self.write_delay(0, input_l + fb_l * self.feedback);
                self.write_delay(1, input_r + fb_r * self.feedback);

                let out_l = input_l + delayed_l * self.mix;
                let out_r = input_r + delayed_r * self.mix;
                (out_l, out_r)
            }

            DelayType::Reverse => {
                // Reversed delay: reads buffer backwards in grains
                let grain_samples = base_delay_samples.max(64.0);

                // Write input
                self.write_delay(0, input_l);
                self.write_delay(1, input_r);

                // Advance grain phase
                self.reverse_grain_phase += 1.0;
                let grain_pos = self.reverse_grain_phase;

                if grain_pos >= grain_samples {
                    self.reverse_grain_phase = 0.0;
                }

                // Backwards through the grain that just went by: at phase p
                // the sample p before the grain's start, which now sits 2p
                // behind the write head.
                let progress = self.reverse_grain_phase / grain_samples;
                let read_offset = 2.0 * progress * grain_samples;

                let delayed_l = self.read_delay(0, read_offset);
                let delayed_r = self.read_delay(1, read_offset * (1.0 + self.stereo_offset));

                // Hann window for smooth grains
                let window = 0.5 * (1.0 - (progress * std::f32::consts::TAU).cos());

                let out_l = input_l + delayed_l * window * self.mix;
                let out_r = input_r + delayed_r * window * self.mix;
                (out_l, out_r)
            }

            DelayType::Ducking => {
                // Ducking delay: delay ducks when input is present
                let input_level = (input_l.abs() + input_r.abs()) * 0.5;

                // Envelope follower: fast attack, slow release
                if input_level > self.duck_env {
                    self.duck_env += (input_level - self.duck_env) * 0.01; // fast attack
                } else {
                    self.duck_env *= 0.9997; // slow release (~200ms)
                }

                // Duck amount: when input is loud, reduce delay mix
                let duck_gain = (1.0 - self.duck_env * 4.0).max(0.0).min(1.0);

                let delay_l = base_delay_samples + tape_mod;
                let delay_r = base_delay_samples * (1.0 + self.stereo_offset) + tape_mod;

                let delayed_l = self.read_delay(0, delay_l);
                let delayed_r = self.read_delay(1, delay_r);

                let fb_l = self.apply_filter(delayed_l, 0);
                let fb_r = self.apply_filter(delayed_r, 1);

                self.write_delay(0, input_l + fb_l * self.feedback);
                self.write_delay(1, input_r + fb_r * self.feedback);

                let out_l = input_l + delayed_l * self.mix * duck_gain;
                let out_r = input_r + delayed_r * self.mix * duck_gain;
                (out_l, out_r)
            }

            DelayType::Diffused => {
                // Diffused delay: echoes smeared through allpass network
                let delay_l = base_delay_samples + tape_mod;
                let delay_r = base_delay_samples * (1.0 + self.stereo_offset) + tape_mod;

                let mut delayed_l = self.read_delay(0, delay_l);
                let mut delayed_r = self.read_delay(1, delay_r);

                // Run through allpass diffusion network
                for i in 0..4 {
                    delayed_l = self.diffuse_ap_l[i].process(delayed_l);
                    delayed_r = self.diffuse_ap_r[i].process(delayed_r);
                }

                let fb_l = self.apply_filter(delayed_l, 0);
                let fb_r = self.apply_filter(delayed_r, 1);

                self.write_delay(0, input_l + fb_l * self.feedback);
                self.write_delay(1, input_r + fb_r * self.feedback);

                let out_l = input_l + delayed_l * self.mix;
                let out_r = input_r + delayed_r * self.mix;
                (out_l, out_r)
            }

            DelayType::Shimmer => {
                // Shimmer delay: pitch-shifted feedback (octave harmonics)
                let delay_l = base_delay_samples + tape_mod;
                let delay_r = base_delay_samples * (1.0 + self.stereo_offset) + tape_mod;

                let delayed_l = self.read_delay(0, delay_l);
                let delayed_r = self.read_delay(1, delay_r);

                // Dual shimmer modulation
                self.shimmer_phase += 1.2 * self.inv_sample_rate;
                if self.shimmer_phase > 1.0 {
                    self.shimmer_phase -= 1.0;
                }
                self.shimmer_phase2 += 1.9 * self.inv_sample_rate;
                if self.shimmer_phase2 > 1.0 {
                    self.shimmer_phase2 -= 1.0;
                }

                let mod1 = fast_sin(self.shimmer_phase * std::f32::consts::TAU) * 0.12;
                let mod2 = fast_sin(self.shimmer_phase2 * std::f32::consts::TAU) * 0.08;

                // Pitch-shifted feedback: read at slightly different positions
                let shift_l = self.read_delay(0, delay_l * (1.0 + mod1)) * 0.3;
                let shift_r = self.read_delay(1, delay_r * (1.0 + mod2)) * 0.3;

                let fb_l = self.apply_filter(delayed_l + shift_l, 0);
                let fb_r = self.apply_filter(delayed_r + shift_r, 1);

                self.write_delay(0, input_l + fb_l * self.feedback);
                self.write_delay(1, input_r + fb_r * self.feedback);

                let out_l = input_l + delayed_l * self.mix;
                let out_r = input_r + delayed_r * self.mix;
                (out_l, out_r)
            }

            DelayType::LoFi => {
                // Lo-Fi delay: bit crushing + sample rate reduction in feedback
                let delay_l = base_delay_samples + tape_mod;
                let delay_r = base_delay_samples * (1.0 + self.stereo_offset) + tape_mod;

                let delayed_l = self.read_delay(0, delay_l);
                let delayed_r = self.read_delay(1, delay_r);

                // Sample rate reduction: hold samples for N cycles
                let reduction = 4.0 + self.stereo_offset * 12.0; // 4x to 16x reduction
                self.lofi_counter += 1.0;
                if self.lofi_counter >= reduction {
                    self.lofi_counter = 0.0;
                    self.lofi_hold[0] = delayed_l;
                    self.lofi_hold[1] = delayed_r;
                }

                // Bit crush: quantize to fewer levels
                let bits: f32 = 8.0; // ~256 levels
                let levels = bits.exp2();
                let crushed_l = (self.lofi_hold[0] * levels).round() / levels;
                let crushed_r = (self.lofi_hold[1] * levels).round() / levels;

                let fb_l = self.apply_filter(crushed_l, 0);
                let fb_r = self.apply_filter(crushed_r, 1);

                self.write_delay(0, input_l + fb_l * self.feedback);
                self.write_delay(1, input_r + fb_r * self.feedback);

                let out_l = input_l + crushed_l * self.mix;
                let out_r = input_r + crushed_r * self.mix;
                (out_l, out_r)
            }
        };

        // Advance write position (stereo interleaved: +2 per sample).
        // Bounded reset instead of `% buffer_size()`.
        self.write_pos += 2;
        let buf_size = self.buffer_size();
        if self.write_pos >= buf_size { self.write_pos -= buf_size; }

        result
    }

    /// Set delay type — resets buffer to prevent artifacts from old state
    pub fn set_delay_type(&mut self, delay_type: DelayType) {
        if self.delay_type != delay_type {
            self.delay_type = delay_type;
            self.reset();
        }
    }

    /// Set delay time in seconds
    pub fn set_time(&mut self, time: f32) {
        self.time = time.max(0.01).min(5.0);
    }

    /// Set feedback
    pub fn set_feedback(&mut self, feedback: f32) {
        self.feedback = feedback.max(0.0).min(0.99);
    }

    /// The delayed signal's level against the dry one, which the delay adds
    /// itself: a send-return delay, not a crossfade.
    pub fn set_level(&mut self, mix: f32) {
        self.mix = mix.max(0.0).min(1.0);
    }

    /// Set tempo sync
    pub fn set_tempo_sync(&mut self, sync: bool) {
        self.tempo_sync = sync;
    }

    /// Set tempo division
    pub fn set_tempo_division(&mut self, division: TempoDivision) {
        self.tempo_division = division;
    }

    /// Set tempo in BPM
    pub fn set_tempo(&mut self, tempo: f32) {
        self.tempo = tempo.max(20.0).min(300.0);
    }

    /// Set stereo offset
    pub fn set_stereo_offset(&mut self, offset: f32) {
        self.stereo_offset = offset.max(0.0).min(1.0);
    }

    /// Set high cut frequency
    pub fn set_high_cut(&mut self, freq: f32) {
        self.high_cut = freq.max(100.0).min(20000.0);
        self.cached_lp_alpha =
            1.0 - (-2.0 * std::f32::consts::PI * self.high_cut / self.sample_rate).exp();
    }

    /// Set low cut frequency
    pub fn set_low_cut(&mut self, freq: f32) {
        self.low_cut = freq.max(20.0).min(5000.0);
        self.cached_hp_alpha =
            1.0 - (-2.0 * std::f32::consts::PI * self.low_cut / self.sample_rate).exp();
    }

    /// Reset delay buffer
    pub fn reset(&mut self) {
        for sample in self.delay_buffer.iter_mut() {
            *sample = 0.0;
        }
        self.write_pos = 0;
        self.hp_state = [0.0; 2];
        self.lp_state = [0.0; 2];
        self.lfo_phase_wow = 0.0;
        self.lfo_phase_flutter = 0.0;
        self.analog_drift_phase = 0.0;
        self.analog_lp_state = [0.0; 2];
        self.reverse_grain_phase = 0.0;
        self.duck_env = 0.0;
        for ap in self
            .diffuse_ap_l
            .iter_mut()
            .chain(self.diffuse_ap_r.iter_mut())
        {
            ap.reset();
        }
        self.shimmer_phase = 0.0;
        self.shimmer_phase2 = 0.0;
        self.lofi_hold = [0.0; 2];
        self.lofi_counter = 0.0;
    }
}

pub static PARAMS: [ParamSpec; 9] = [
    ParamSpec { id: "type", name: "Type", short: "Type", kind: ParamKind::Enum { variants: &TYPES, labels: &TYPE_LABELS }, unit: Unit::None, default: Value::E("digital"), flags: ParamFlags::NONE },
    ParamSpec { id: "time", name: "Time", short: "Time", kind: ParamKind::Float { min: 0.01, max: 2.0, curve: Curve::Log }, unit: Unit::Seconds, default: Value::F(0.3), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "feedback", name: "Feedback", short: "Fdbk", kind: ParamKind::Float { min: 0.0, max: 0.95, curve: Curve::Linear }, unit: Unit::Percent, default: Value::F(0.4), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "level", name: "Level", short: "Level", kind: ParamKind::Float { min: 0.0, max: 1.0, curve: Curve::Linear }, unit: Unit::Percent, default: Value::F(0.5), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "stereo", name: "Stereo Offset", short: "Ster", kind: ParamKind::Float { min: 0.0, max: 1.0, curve: Curve::Linear }, unit: Unit::Percent, default: Value::F(0.2), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "high-cut", name: "High Cut", short: "HiCut", kind: ParamKind::Float { min: 500.0, max: 20000.0, curve: Curve::Log }, unit: Unit::Hz, default: Value::F(12000.0), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "low-cut", name: "Low Cut", short: "LoCut", kind: ParamKind::Float { min: 20.0, max: 2000.0, curve: Curve::Log }, unit: Unit::Hz, default: Value::F(20.0), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "sync", name: "Tempo Sync", short: "Sync", kind: ParamKind::Bool, unit: Unit::None, default: Value::B(false), flags: ParamFlags::NONE },
    ParamSpec { id: "division", name: "Division", short: "Div", kind: ParamKind::Enum { variants: &super::tempo::DIVISIONS, labels: &super::tempo::DIVISION_LABELS }, unit: Unit::None, default: Value::E("1-4"), flags: ParamFlags::NONE },
];

pub static SPEC: EffectSpec = EffectSpec {
    kind: "delay",
    name: "Delay",
    category: Category::Delay,
    params: &PARAMS,
    readouts: &[],
    needs: Needs::TEMPO,
};

pub fn build(sample_rate: f32) -> Box<dyn Effect> {
    Box::new(Delay::new(sample_rate))
}

impl Effect for Delay {
    fn spec(&self) -> &'static EffectSpec {
        &SPEC
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.set_delay_type(DelayType::named(value.as_variant().unwrap_or("digital"))),
            1 => self.set_time(value.as_f32()),
            2 => self.set_feedback(value.as_f32()),
            3 => self.set_level(value.as_f32()),
            4 => self.set_stereo_offset(value.as_f32()),
            5 => self.set_high_cut(value.as_f32()),
            6 => self.set_low_cut(value.as_f32()),
            7 => self.set_tempo_sync(value.as_bool()),
            8 => self.set_tempo_division(TempoDivision::named(value.as_variant().unwrap_or("1-4"))),
            _ => {}
        }
    }

    fn param(&self, index: usize) -> Value {
        match index {
            0 => Value::E(self.delay_type.name()),
            1 => Value::F(self.time),
            2 => Value::F(self.feedback),
            3 => Value::F(self.mix),
            4 => Value::F(self.stereo_offset),
            5 => Value::F(self.high_cut),
            6 => Value::F(self.low_cut),
            7 => Value::B(self.tempo_sync),
            _ => Value::E(self.tempo_division.name()),
        }
    }

    fn process(&mut self, ports: &mut Ports<'_>) {
        self.set_tempo(ports.transport.bpm);
        for i in 0..ports.frames() {
            let (l, r) = self.tick(ports.audio.l[i], ports.audio.r[i]);
            ports.audio.l[i] = l;
            ports.audio.r[i] = r;
        }
    }

    fn reset(&mut self) {
        Delay::reset(self)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        *self = Delay::new(sample_rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::{StereoMut, Transport};

    fn run(d: &mut Delay, l: &mut [f32], r: &mut [f32], bpm: f32) {
        let mut ports = Ports { audio: StereoMut { l, r }, buses: &[], sidechain: None, modulator: None, transport: Transport { bpm, beat: 0.0, playing: true }, musical: Default::default(), sample_rate: 48_000.0 };
        d.process(&mut ports);
    }

    #[test]
    fn the_spec_is_sound_and_its_size_is_frozen() {
        assert!(SPEC.problems().is_empty(), "{:?}", SPEC.problems());
        assert_eq!(SPEC.params.len(), 9);
        assert_eq!(TYPES.len(), DelayType::ALL.len());
    }

    /// An impulse comes back once per delay time, quieter each time.
    #[test]
    fn an_echo_lands_where_the_time_says() {
        let mut d = Delay::new(48_000.0);
        d.set_param(1, Value::F(0.1));
        d.set_param(2, Value::F(0.5));
        d.set_param(3, Value::F(1.0));
        d.set_param(5, Value::F(20_000.0));
        let n = 48_000 / 2;
        let mut l = vec![0.0_f32; n];
        let mut r = vec![0.0_f32; n];
        l[0] = 1.0;
        r[0] = 1.0;
        run(&mut d, &mut l, &mut r, 120.0);
        let peak_near = |t: usize| l[t - 20..t + 20].iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        let first = peak_near(4800);
        let second = peak_near(9600);
        assert!(first > 0.3, "no echo at 100 ms: {first}");
        assert!(second > 0.1 && second < first, "feedback: {second} after {first}");
    }

    /// Synced, the time is a beat at the transport's tempo.
    #[test]
    fn a_synced_delay_follows_the_transport() {
        let mut d = Delay::new(48_000.0);
        d.set_param(7, Value::B(true));
        d.set_param(8, Value::E("1-4"));
        d.set_param(3, Value::F(1.0));
        d.set_param(5, Value::F(20_000.0));
        let n = 48_000;
        let mut l = vec![0.0_f32; n];
        let mut r = vec![0.0_f32; n];
        l[0] = 1.0;
        r[0] = 1.0;
        run(&mut d, &mut l, &mut r, 120.0);
        let at_beat = l[24_000 - 20..24_000 + 20].iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(at_beat > 0.3, "no echo on the beat at 120 bpm: {at_beat}");
    }

    #[test]
    fn every_type_is_finite_and_produces_a_tail() {
        for t in TYPES {
            let mut d = Delay::new(48_000.0);
            d.set_param(0, Value::E(t));
            d.set_param(1, Value::F(0.1));
            d.set_param(3, Value::F(1.0));
            let n = 48_000;
            let mut l = vec![0.0_f32; n];
            let mut r = vec![0.0_f32; n];
            for i in 0..480 { l[i] = (i as f32 * 0.1).sin() * 0.5; r[i] = l[i]; }
            run(&mut d, &mut l, &mut r, 120.0);
            assert!(l.iter().chain(r.iter()).all(|v| v.is_finite()), "{t}");
            // Anything after the burst is the delay's own.
            let tail: f32 = l[600..].iter().map(|v| v.abs()).sum();
            assert!(tail > 0.0, "{t} has no tail");
        }
    }
}
