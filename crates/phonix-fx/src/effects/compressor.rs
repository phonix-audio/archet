//! A stereo-linked compressor in five characters: VCA, opto, FET, a
//! three-band split, and a hard-knee feed-forward bus mode with no
//! lookahead. The other four read 5 ms ahead.

use crate::effect::{Effect, Ports, Value};
use super::math::*;
use crate::spec::{Category, Curve, EffectSpec, Needs, ParamFlags, ParamKind, ParamSpec, ReadoutKind, ReadoutSpec, Unit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressorMode {
    Vca,
    Opto,
    Fet,
    Multiband,
    Bus,
}

const MODES: [&str; 5] = ["vca", "opto", "fet", "multiband", "bus"];
const MODE_LABELS: [&str; 5] = ["VCA", "Opto", "FET", "Multiband", "Bus"];

impl CompressorMode {
    const ALL: [CompressorMode; 5] = [Self::Vca, Self::Opto, Self::Fet, Self::Multiband, Self::Bus];

    fn named(name: &str) -> Self {
        MODES.iter().position(|m| *m == name).map(|i| Self::ALL[i]).unwrap_or(Self::Vca)
    }

    fn name(self) -> &'static str {
        MODES[Self::ALL.iter().position(|m| *m == self).unwrap_or(0)]
    }
}

pub struct Compressor {
    sample_rate: f32,
    mode: CompressorMode,
    threshold: f32,   // dB (-60 to 0)
    ratio: f32,       // 1:1 to 20:1
    attack: f32,      // seconds
    release: f32,     // seconds
    makeup_gain: f32, // dB
    knee: f32,        // dB width of soft knee

    // State
    envelope_db: f32, // smoothed level in dB
    attack_coef: f32,
    release_coef: f32,

    // Sidechain: when active, the detector listens to the bus the host
    // handed over instead of the input audio.
    sidechain_active: bool,
    sidechain_l: f32,
    sidechain_r: f32,

    // Multiband state
    mb_lo_state: f32, // 1-pole LPF state for low crossover
    mb_hi_state: f32, // 1-pole LPF state for high crossover
    mb_env_lo: f32,   // per-band envelope (dB)
    mb_env_mid: f32,
    mb_env_hi: f32,
    mb_lo_coef: f32, // cached crossover coefficient at 200Hz
    mb_hi_coef: f32, // cached crossover coefficient at 3000Hz

    // === Phase 2 upgrades ===
    /// Lookahead delay line (5 ms): the audio path reads `lookahead_len`
    /// samples behind the detector so the envelope can start opening
    /// before a transient arrives.
    lookahead_l: Vec<f32>,
    lookahead_r: Vec<f32>,
    lookahead_pos: usize,
    lookahead_len: usize,
    /// Sidechain high-pass filter cutoff in Hz — stops bass from pumping
    /// the whole mix. 0 disables.
    sc_hpf_hz: f32,
    sc_hp_l_prev_in: f32,
    sc_hp_l_prev_out: f32,
    sc_hp_r_prev_in: f32,
    sc_hp_r_prev_out: f32,
    /// When non-zero, auto-makeup is added on top of the user-set makeup.
    auto_makeup: f32, // 0..1

    // ── Control-rate cache for Opto/FET modes ─────────────────
    // process_opto + process_fet recomputed `(-1/(τ·sr)).exp()`
    // twice per sample. The τ depends on `envelope_db` (slow,
    // one-pole follower) — sample-accurate refresh isn't needed.
    // These get refreshed every CMP_CTRL_DIVIDER samples; first
    // sample after a mode/attack/release change forces a refresh
    // (`ctrl_dirty`).
    ctrl_counter: u32,
    ctrl_dirty: bool,
    cached_opto_att: f32,
    cached_opto_rel: f32,
    cached_fet_att: f32,
    cached_fet_rel: f32,

    // ── Bus mode (verbatim canon `compress()`): a LINEAR peak follower
    // (not the dB gain-reduction envelope the other modes smooth) plus the
    // linear-domain threshold / makeup caches refreshed in update_coefficients.
    bus_env: f32,
    bus_thr_lin: f32,
    bus_mk: f32,
}

const CMP_CTRL_DIVIDER: u32 = 32;

impl Compressor {
    pub fn new(sample_rate: f32) -> Self {
        let lookahead_len = ((sample_rate * 0.005) as usize).max(1);
        let mut s = Self {
            sample_rate,
            mode: CompressorMode::Vca,
            threshold: -18.0,
            ratio: 4.0,
            attack: 0.005,
            release: 0.1,
            makeup_gain: 0.0,
            knee: 6.0,
            // `envelope_db` is the SMOOTHED gain-reduction in dB (added to makeup to form the
            // output gain: gain = db_to_lin(envelope_db + makeup)). Its REST value is 0 (no
            // reduction = unity), matching mb_env_* below. Initialising it to -96 made every
            // compressor start FULLY CLOSED (gain ~= db_to_lin(-96) ~= silence) and open over
            // ~5x its release time -> an audible fade-in at the very start of an offline bounce
            // (and at each instrument entrance for a freshly-constructed strip). Init at 0 = unity.
            envelope_db: 0.0,
            attack_coef: 0.0,
            release_coef: 0.0,
            sidechain_active: false,
            sidechain_l: 0.0,
            sidechain_r: 0.0,
            mb_lo_state: 0.0,
            mb_hi_state: 0.0,
            mb_env_lo: 0.0,
            mb_env_mid: 0.0,
            mb_env_hi: 0.0,
            mb_lo_coef: (-std::f32::consts::TAU * 200.0 / sample_rate).exp(),
            mb_hi_coef: (-std::f32::consts::TAU * 3000.0 / sample_rate).exp(),
            lookahead_l: vec![0.0; lookahead_len],
            lookahead_r: vec![0.0; lookahead_len],
            lookahead_pos: 0,
            lookahead_len,
            sc_hpf_hz: 0.0,
            sc_hp_l_prev_in: 0.0, sc_hp_l_prev_out: 0.0,
            sc_hp_r_prev_in: 0.0, sc_hp_r_prev_out: 0.0,
            auto_makeup: 0.0,
            ctrl_counter: 0,
            ctrl_dirty: true,
            cached_opto_att: 0.0,
            cached_opto_rel: 0.0,
            cached_fet_att: 0.0,
            cached_fet_rel: 0.0,
            bus_env: 0.0,
            bus_thr_lin: 1.0,
            bus_mk: 1.0,
        };
        s.update_coefficients();
        s
    }

    /// Apply the 1-pole sidechain HPF to the detector input. Returns
    /// the passed-through value when `sc_hpf_hz` is 0.
    #[inline]
    fn sc_hp_filter(&mut self, x_l: f32, x_r: f32) -> (f32, f32) {
        if self.sc_hpf_hz <= 0.1 { return (x_l, x_r); }
        // 1-pole RC HPF: y = alpha * (y_prev + x - x_prev)
        let rc = 1.0 / (std::f32::consts::TAU * self.sc_hpf_hz);
        let dt = 1.0 / self.sample_rate;
        let alpha = rc / (rc + dt);
        let out_l = alpha * (self.sc_hp_l_prev_out + x_l - self.sc_hp_l_prev_in);
        let out_r = alpha * (self.sc_hp_r_prev_out + x_r - self.sc_hp_r_prev_in);
        self.sc_hp_l_prev_in = x_l;
        self.sc_hp_r_prev_in = x_r;
        self.sc_hp_l_prev_out = out_l;
        self.sc_hp_r_prev_out = out_r;
        (out_l, out_r)
    }

    /// Push a new stereo sample into the lookahead buffer and return the
    /// sample that falls out of the far end (delayed by `lookahead_len`).
    #[inline]
    fn lookahead_push(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.lookahead_len <= 1 { return (l, r); }
        let out_l = self.lookahead_l[self.lookahead_pos];
        let out_r = self.lookahead_r[self.lookahead_pos];
        self.lookahead_l[self.lookahead_pos] = l;
        self.lookahead_r[self.lookahead_pos] = r;
        self.lookahead_pos = (self.lookahead_pos + 1) % self.lookahead_len;
        (out_l, out_r)
    }

    #[inline]
    fn auto_makeup_db(&self) -> f32 {
        // Classic rule: makeup = -threshold * (1 - 1/ratio) * 0.5
        let ratio_inv = 1.0 / self.ratio.max(1.0);
        (-self.threshold * (1.0 - ratio_inv) * 0.5) * self.auto_makeup
    }

    fn update_coefficients(&mut self) {
        self.attack_coef = (-1.0 / (self.attack * self.sample_rate)).exp();
        self.release_coef = (-1.0 / (self.release * self.sample_rate)).exp();
    }

    fn gain_computer(&self, input_db: f32) -> f32 {
        let half_knee = self.knee * 0.5;
        let diff = input_db - self.threshold;

        if diff <= -half_knee {
            // Below knee: no compression
            input_db
        } else if diff >= half_knee {
            // Above knee: full compression
            self.threshold + diff / self.ratio
        } else {
            // In knee: smooth transition
            let x = diff + half_knee;
            input_db + (1.0 / self.ratio - 1.0) * x * x / (2.0 * self.knee)
        }
    }

    fn tick(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        match self.mode {
            CompressorMode::Vca => self.process_vca(input_l, input_r),
            CompressorMode::Opto => self.process_opto(input_l, input_r),
            CompressorMode::Fet => self.process_fet(input_l, input_r),
            CompressorMode::Multiband => self.process_multiband(input_l, input_r),
            CompressorMode::Bus => self.process_bus(input_l, input_r),
        }
    }

    // Bus: a VERBATIM port of the canon render's per-stem `compress()` — a
    // feed-forward PEAK detector (stereo-linked, max|L|,|R|), one-pole
    // attack/release on the raw level, a HARD-knee LINEAR-domain gain
    // `(thr + (env-thr)/ratio)/env`, then makeup. NO lookahead (so it does not
    // delay the audio path and phase-skew against uncompressed stems) and NO
    // soft knee — exactly the render's topology, so the .phx per-track
    // compression is faithful to the bespoke render instead of merely "close".
    fn process_bus(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        // threshold/makeup change only on a param edit; refresh their linear
        // caches every CTRL_DIVIDER samples (and on the first sample) — the
        // hot path then has no powf. (update_coefficients also seeds them.)
        if self.ctrl_counter == 0 || self.ctrl_dirty {
            self.bus_thr_lin = 10f32.powf(self.threshold / 20.0);
            self.bus_mk = 10f32.powf(self.makeup_gain / 20.0);
            self.ctrl_dirty = false;
        }
        self.ctrl_counter = (self.ctrl_counter + 1) % CMP_CTRL_DIVIDER;

        let (det_l, det_r) = if self.sidechain_active {
            (self.sidechain_l, self.sidechain_r)
        } else {
            (input_l, input_r)
        };
        let pk = det_l.abs().max(det_r.abs());
        // attack_coef/release_coef are exp(-1/(t·sr)) == the render's ga/gr.
        let coef = if pk > self.bus_env { self.attack_coef } else { self.release_coef };
        self.bus_env = coef * self.bus_env + (1.0 - coef) * pk;
        let g = if self.bus_env > self.bus_thr_lin {
            (self.bus_thr_lin + (self.bus_env - self.bus_thr_lin) / self.ratio) / self.bus_env.max(1e-9)
        } else {
            1.0
        };
        let gm = g * self.bus_mk;
        let wet_l = input_l * gm;
        let wet_r = input_r * gm;
        (
            wet_l,
            wet_r,
        )
    }

    /// Derive the detector sample (after sidechain HPF when enabled) and
    /// the delayed audio sample for the specified input pair. Used by all
    /// four compressor modes.
    #[inline]
    fn prepare_lookahead(&mut self, input_l: f32, input_r: f32) -> (f32, f32, f32, f32) {
        let (det_l, det_r) = if self.sidechain_active {
            self.sc_hp_filter(self.sidechain_l, self.sidechain_r)
        } else {
            self.sc_hp_filter(input_l, input_r)
        };
        let (aud_l, aud_r) = self.lookahead_push(input_l, input_r);
        (det_l, det_r, aud_l, aud_r)
    }

    // VCA: clean, transparent, SSL-style. Standard feedforward design.
    fn process_vca(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        let (det_l, det_r, aud_l, aud_r) = self.prepare_lookahead(input_l, input_r);
        let peak = det_l.abs().max(det_r.abs()).max(1e-10);
        let input_db = 20.0 * fast_log10(peak);
        let gain_reduction_db = self.gain_computer(input_db) - input_db;

        let coef = if gain_reduction_db < self.envelope_db {
            self.attack_coef
        } else {
            self.release_coef
        };
        self.envelope_db = coef * self.envelope_db + (1.0 - coef) * gain_reduction_db;

        let total_makeup = self.makeup_gain + self.auto_makeup_db();
        let gain = fast_db_to_lin(self.envelope_db + total_makeup).min(128.0);
        let wet_l = aud_l * gain;
        let wet_r = aud_r * gain;
        (
            wet_l,
            wet_r,
        )
    }

    // Opto: program-dependent timing (LA-2A style). Slow attack on transients, faster on sustained.
    fn process_opto(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        let (det_l, det_r, aud_l, aud_r) = self.prepare_lookahead(input_l, input_r);
        let peak = det_l.abs().max(det_r.abs()).max(1e-10);
        let input_db = 20.0 * fast_log10(peak);
        let gain_reduction_db = self.gain_computer(input_db) - input_db;

        // Program-dependent: attack slows as GR deepens, release has two stages
        // Opto attack/release depend on `gr_depth` (envelope follower —
        // slow). Refresh the two exp() coefs only every CTRL_DIVIDER
        // samples; the dependency on gr_depth doesn't need to be
        // sample-accurate.
        if self.ctrl_counter == 0 || self.ctrl_dirty {
            let gr_depth = (-self.envelope_db).max(0.0);
            let opto_attack = self.attack * (1.0 + gr_depth * 0.15);
            let opto_release = if gr_depth > 10.0 {
                self.release * 3.0
            } else {
                self.release * (1.0 + gr_depth * 0.1)
            };
            self.cached_opto_att = (-1.0 / (opto_attack * self.sample_rate)).exp();
            self.cached_opto_rel = (-1.0 / (opto_release * self.sample_rate)).exp();
            self.ctrl_dirty = false;
        }
        self.ctrl_counter = (self.ctrl_counter + 1) % CMP_CTRL_DIVIDER;

        let coef = if gain_reduction_db < self.envelope_db {
            self.cached_opto_att
        } else {
            self.cached_opto_rel
        };
        self.envelope_db = coef * self.envelope_db + (1.0 - coef) * gain_reduction_db;

        let total_makeup = self.makeup_gain + self.auto_makeup_db();
        let gain = fast_db_to_lin(self.envelope_db + total_makeup).min(128.0);
        let wet_l = aud_l * gain;
        let wet_r = aud_r * gain;
        (
            wet_l,
            wet_r,
        )
    }

    // FET: fast, aggressive (1176 style). Hard knee, feedback topology emulation, all-buttons character.
    fn process_fet(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        let (det_l, det_r, aud_l, aud_r) = self.prepare_lookahead(input_l, input_r);
        let peak = det_l.abs().max(det_r.abs()).max(1e-10);
        let input_db = 20.0 * fast_log10(peak);

        // FET uses hard knee
        let diff = input_db - self.threshold;
        let output_db = if diff > 0.0 {
            self.threshold + diff / self.ratio
        } else {
            input_db
        };
        let gain_reduction_db = output_db - input_db;

        // FET attack is fully static (depends only on the `attack`
        // knob × 0.2). Release depends on `gr_depth` — slow. Same
        // control-rate cache pattern as Opto.
        if self.ctrl_counter == 0 || self.ctrl_dirty {
            let fet_attack = self.attack * 0.2;
            let fet_release = self.release * (1.0 + (-self.envelope_db).max(0.0) * 0.05);
            self.cached_fet_att = (-1.0 / (fet_attack * self.sample_rate)).exp();
            self.cached_fet_rel = (-1.0 / (fet_release * self.sample_rate)).exp();
            self.ctrl_dirty = false;
        }
        self.ctrl_counter = (self.ctrl_counter + 1) % CMP_CTRL_DIVIDER;

        let coef = if gain_reduction_db < self.envelope_db {
            self.cached_fet_att
        } else {
            self.cached_fet_rel
        };
        self.envelope_db = coef * self.envelope_db + (1.0 - coef) * gain_reduction_db;

        // FET adds harmonic saturation
        let total_makeup = self.makeup_gain + self.auto_makeup_db();
        let gain = fast_db_to_lin(self.envelope_db + total_makeup).min(128.0);
        let wet_l = Self::fet_saturate(aud_l * gain);
        let wet_r = Self::fet_saturate(aud_r * gain);
        (
            wet_l,
            wet_r,
        )
    }

    fn fet_saturate(x: f32) -> f32 {
        // Soft asymmetric saturation for FET character
        if x > 0.0 {
            (x * 1.5).tanh() / 1.5_f32.tanh()
        } else {
            (x * 1.2).tanh() / 1.2_f32.tanh()
        }
    }

    // Multiband: split into 3 bands (low/mid/high) using proper 1-pole filters, compress independently, sum
    fn process_multiband(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        // Lookahead (SC HPF doesn't apply per-band, but we still delay the audio
        // path so the audio stays in sync with other modes).
        let (_det_l, _det_r, input_l, input_r) = self.prepare_lookahead(input_l, input_r);
        let mono = (input_l + input_r) * 0.5;

        // Low band: 1-pole LPF at 200 Hz (cached coefficient)
        self.mb_lo_state = self.mb_lo_coef * self.mb_lo_state + (1.0 - self.mb_lo_coef) * mono;
        let lo = self.mb_lo_state;

        // High-pass at 3kHz: input minus low-pass at 3kHz (cached coefficient)
        self.mb_hi_state = self.mb_hi_coef * self.mb_hi_state + (1.0 - self.mb_hi_coef) * mono;
        let hi = mono - self.mb_hi_state;

        // Mid: remainder
        let mid = mono - lo - hi;

        // Compress each band with envelope smoothing
        let ratio = self.ratio;
        let att = self.attack_coef;
        let rel = self.release_coef;

        let compress = |input: f32, threshold: f32, env: &mut f32| -> f32 {
            let level = input.abs().max(1e-10);
            let db = 20.0 * fast_log10(level);
            let diff = db - threshold;
            let gr_db = if diff > 0.0 {
                -(diff - diff / ratio)
            } else {
                0.0
            };
            let coef = if gr_db < *env { att } else { rel };
            *env = coef * *env + (1.0 - coef) * gr_db;
            input * fast_db_to_lin(*env)
        };

        let lo_compressed = compress(lo, self.threshold - 6.0, &mut self.mb_env_lo);
        let mid_compressed = compress(mid, self.threshold, &mut self.mb_env_mid);
        let hi_compressed = compress(hi, self.threshold + 3.0, &mut self.mb_env_hi);

        let wet = lo_compressed + mid_compressed + hi_compressed;
        let gain = fast_db_to_lin(self.makeup_gain).min(128.0);
        let wet_l = wet * gain;
        let wet_r = wet * gain;

        // Preserve stereo image
        let sum_abs = input_l.abs() + input_r.abs();
        let width = if sum_abs > 1e-10 {
            (input_l - input_r) / (sum_abs + 1e-10)
        } else {
            0.0
        };
        let out_l = wet_l * (1.0 + width * 0.5);
        let out_r = wet_r * (1.0 - width * 0.5);

        (
            out_l,
            out_r,
        )
    }

    pub fn reset(&mut self) {
        // At REST a gain-reduction envelope is 0 dB — no reduction. This said
        // -96, which is 96 dB of gain reduction: a reset compressor came back
        // silent and took its whole release time to let go. The constructor
        // already says so twenty lines above; only the reset disagreed.
        self.envelope_db = 0.0;
        self.mb_lo_state = 0.0;
        self.mb_hi_state = 0.0;
        self.mb_env_lo = 0.0;
        self.mb_env_mid = 0.0;
        self.mb_env_hi = 0.0;
        for s in &mut self.lookahead_l { *s = 0.0; }
        for s in &mut self.lookahead_r { *s = 0.0; }
        self.lookahead_pos = 0;
        self.sc_hp_l_prev_in = 0.0; self.sc_hp_l_prev_out = 0.0;
        self.sc_hp_r_prev_in = 0.0; self.sc_hp_r_prev_out = 0.0;
    }

    pub fn set_threshold(&mut self, db: f32) {
        self.threshold = db.clamp(-60.0, 0.0);
    }
    pub fn set_ratio(&mut self, r: f32) {
        self.ratio = r.clamp(1.0, 20.0);
    }
    pub fn set_attack(&mut self, s: f32) {
        self.attack = s.clamp(0.0001, 0.5);
        self.update_coefficients();
        self.ctrl_dirty = true;
    }
    pub fn set_release(&mut self, s: f32) {
        self.release = s.clamp(0.01, 2.0);
        self.update_coefficients();
        self.ctrl_dirty = true;
    }
    pub fn set_makeup_gain(&mut self, db: f32) {
        self.makeup_gain = db.clamp(0.0, 24.0);
    }
    pub fn set_knee(&mut self, db: f32) {
        self.knee = db.clamp(0.0, 24.0);
    }
    pub fn set_mode(&mut self, mode: CompressorMode) {
        self.mode = mode;
        self.ctrl_dirty = true;
    }
    /// Sidechain HPF cutoff in Hz. Set to 0 to disable. 20..500 is typical.
    pub fn set_sc_hpf(&mut self, hz: f32) { self.sc_hpf_hz = hz.clamp(0.0, 2000.0); }
    /// Auto-makeup amount 0..1 (0 = off, 1 = full rule-of-thumb boost).
    pub fn set_auto_makeup(&mut self, v: f32) { self.auto_makeup = v.clamp(0.0, 1.0); }
}

pub static PARAMS: [ParamSpec; 9] = [
    ParamSpec { id: "threshold", name: "Threshold", short: "Thresh", kind: ParamKind::Float { min: -60.0, max: 0.0, curve: Curve::Linear }, unit: Unit::Db, default: Value::F(-18.0), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "ratio", name: "Ratio", short: "Ratio", kind: ParamKind::Float { min: 1.0, max: 20.0, curve: Curve::Log }, unit: Unit::Ratio, default: Value::F(4.0), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "attack", name: "Attack", short: "Atk", kind: ParamKind::Float { min: 0.0001, max: 0.5, curve: Curve::Log }, unit: Unit::Seconds, default: Value::F(0.005), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "release", name: "Release", short: "Rel", kind: ParamKind::Float { min: 0.01, max: 2.0, curve: Curve::Log }, unit: Unit::Seconds, default: Value::F(0.1), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "makeup", name: "Makeup", short: "Makeup", kind: ParamKind::Float { min: 0.0, max: 24.0, curve: Curve::Linear }, unit: Unit::Db, default: Value::F(0.0), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "knee", name: "Knee", short: "Knee", kind: ParamKind::Float { min: 0.0, max: 24.0, curve: Curve::Linear }, unit: Unit::Db, default: Value::F(6.0), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "mode", name: "Mode", short: "Mode", kind: ParamKind::Enum { variants: &MODES, labels: &MODE_LABELS }, unit: Unit::None, default: Value::E("vca"), flags: ParamFlags::NONE },
    ParamSpec { id: "sidechain-hpf", name: "Sidechain High-Pass", short: "SC HPF", kind: ParamKind::Float { min: 0.0, max: 2000.0, curve: Curve::Linear }, unit: Unit::Hz, default: Value::F(0.0), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "auto-makeup", name: "Auto Makeup", short: "Auto", kind: ParamKind::Float { min: 0.0, max: 1.0, curve: Curve::Linear }, unit: Unit::Percent, default: Value::F(0.0), flags: ParamFlags::AUTOMATABLE },
];

pub static READOUTS: [ReadoutSpec; 1] = [
    ReadoutSpec { id: "gain-reduction", name: "Gain Reduction", kind: ReadoutKind::Scalar, unit: Unit::Db },
];

pub static SPEC: EffectSpec = EffectSpec {
    kind: "compressor",
    name: "Compressor",
    category: Category::Dynamics,
    params: &PARAMS,
    readouts: &READOUTS,
    needs: Needs::SIDECHAIN,
};

pub fn build(sample_rate: f32) -> Box<dyn Effect> {
    Box::new(Compressor::new(sample_rate))
}

impl Effect for Compressor {
    fn spec(&self) -> &'static EffectSpec {
        &SPEC
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.set_threshold(value.as_f32()),
            1 => self.set_ratio(value.as_f32()),
            2 => self.set_attack(value.as_f32()),
            3 => self.set_release(value.as_f32()),
            4 => self.set_makeup_gain(value.as_f32()),
            5 => self.set_knee(value.as_f32()),
            6 => self.set_mode(CompressorMode::named(value.as_variant().unwrap_or("vca"))),
            7 => self.set_sc_hpf(value.as_f32()),
            8 => self.set_auto_makeup(value.as_f32()),
            _ => {}
        }
    }

    fn param(&self, index: usize) -> Value {
        match index {
            0 => Value::F(self.threshold),
            1 => Value::F(self.ratio),
            2 => Value::F(self.attack),
            3 => Value::F(self.release),
            4 => Value::F(self.makeup_gain),
            5 => Value::F(self.knee),
            6 => Value::E(self.mode.name()),
            7 => Value::F(self.sc_hpf_hz),
            _ => Value::F(self.auto_makeup),
        }
    }

    fn process(&mut self, ports: &mut Ports<'_>) {
        let n = ports.frames();
        let sidechain = ports.sidechain().map(|s| (s.l, s.r));
        match sidechain {
            Some((sl, sr)) => {
                self.sidechain_active = true;
                for i in 0..n {
                    self.sidechain_l = sl.get(i).copied().unwrap_or(0.0);
                    self.sidechain_r = sr.get(i).copied().unwrap_or(0.0);
                    let (l, r) = self.tick(ports.audio.l[i], ports.audio.r[i]);
                    ports.audio.l[i] = l;
                    ports.audio.r[i] = r;
                }
            }
            None => {
                self.sidechain_active = false;
                for i in 0..n {
                    let (l, r) = self.tick(ports.audio.l[i], ports.audio.r[i]);
                    ports.audio.l[i] = l;
                    ports.audio.r[i] = r;
                }
            }
        }
    }

    fn reset(&mut self) {
        Compressor::reset(self)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        *self = Compressor::new(sample_rate);
    }

    fn latency_samples(&self) -> usize {
        if self.mode == CompressorMode::Bus || self.lookahead_len <= 1 { 0 } else { self.lookahead_len }
    }

    fn readout(&self, index: usize) -> f32 {
        match index {
            0 => self.envelope_db.min(0.0),
            _ => f32::NAN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_is_sound_and_its_size_is_frozen() {
        assert!(SPEC.problems().is_empty(), "{:?}", SPEC.problems());
        assert_eq!(SPEC.params.len(), 9);
    }

    #[test]
    fn bus_mode_adds_no_latency_and_the_others_read_ahead() {
        let mut c = Compressor::new(48_000.0);
        assert_eq!(c.latency_samples(), 240);
        c.set_param(6, Value::E("bus"));
        assert_eq!(c.latency_samples(), 0);
        assert_eq!(c.param(6), Value::E("bus"));
    }

    #[test]
    fn a_loud_tone_comes_out_quieter_than_it_went_in() {
        let mut c = Compressor::new(48_000.0);
        c.set_param(6, Value::E("bus"));
        c.set_param(0, Value::F(-20.0));
        c.set_param(1, Value::F(4.0));
        let mut l: Vec<f32> = (0..4800).map(|i| 0.9 * (i as f32 * 0.05).sin()).collect();
        let mut r = l.clone();
        let mut ports = Ports { audio: crate::effect::StereoMut { l: &mut l, r: &mut r }, buses: &[], sidechain: None, modulator: None, transport: Default::default(), musical: Default::default(), sample_rate: 48_000.0 };
        c.process(&mut ports);
        let peak = l[2400..].iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(peak < 0.6, "{peak}");
    }
}
