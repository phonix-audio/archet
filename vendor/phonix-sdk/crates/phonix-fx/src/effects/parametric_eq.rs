//! Four bands of RBJ biquad, in series: shelf, two peaks, shelf by default,
//! any of them a peak, a shelf or a low-pass.

use phonix_dsp::filters::Biquad;

use crate::effect::{Effect, Ports, Value};
use crate::spec::{Category, Curve, EffectSpec, Needs, ParamFlags, ParamKind, ParamSpec, Unit};

pub struct ParametricEq {
    sample_rate: f32,

    // Per-band state (stereo pair)
    bands_l: [Biquad; 4],
    bands_r: [Biquad; 4],

    // Per band: freq, gain (dB), q, enabled
    freqs:    [f32; 4],
    gains_db: [f32; 4],
    qs:       [f32; 4],
    enabled:  [bool; 4],
    /// Per-band filter type. 0 = AUTO (position default: band0 low-shelf, band3 high-shelf,
    /// bands 1/2 peaking) so existing patches are byte-identical. 1=peak, 2=low-shelf,
    /// 3=high-shelf, 4=low-pass. Lets a 3-peak EQ (e.g. the canon drum bus 90/4500/9500) be
    /// reproduced EXACTLY instead of being forced into the shelf-at-the-edges layout.
    types:    [u8; 4],
}

impl ParametricEq {
    pub fn new(sample_rate: f32) -> Self {
        let mut s = Self {
            sample_rate,
            bands_l: [Biquad::new(); 4],
            bands_r: [Biquad::new(); 4],
            freqs: [100.0, 500.0, 2000.0, 8000.0],
            gains_db: [0.0; 4],
            qs: [0.707, 0.707, 0.707, 0.707],
            enabled: [true; 4],
            types: [0; 4], // all AUTO = the historical position-default layout
        };
        for b in 0..4 { s.update_band(b); }
        s
    }

    pub fn set_band_type(&mut self, b: usize, t: u8) {
        if b < 4 && self.types[b] != t { self.types[b] = t; self.update_band(b); }
    }

    fn update_band(&mut self, b: usize) {
        let fs = self.sample_rate;
        let f = self.freqs[b];
        let g = self.gains_db[b];
        let q = self.qs[b];
        // Resolve AUTO (0) to the historical per-position default so old patches are unchanged.
        let t = match self.types[b] { 0 => match b { 0 => 2, 3 => 3, _ => 1 }, t => t };
        match t {
            1 => { self.bands_l[b].set_peak(fs, f, q, g);       self.bands_r[b].set_peak(fs, f, q, g); }
            2 => { self.bands_l[b].set_low_shelf(fs, f, q, g);  self.bands_r[b].set_low_shelf(fs, f, q, g); }
            3 => { self.bands_l[b].set_high_shelf(fs, f, q, g); self.bands_r[b].set_high_shelf(fs, f, q, g); }
            4 => { self.bands_l[b].set_lowpass(fs, f, q);       self.bands_r[b].set_lowpass(fs, f, q); }
            _ => { self.bands_l[b].set_peak(fs, f, q, g);       self.bands_r[b].set_peak(fs, f, q, g); }
        }
    }

    fn tick(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        let mut l = input_l;
        let mut r = input_r;
        for b in 0..4 {
            // Process a band when enabled AND it actually shapes the signal: a gain band needs
            // |gain| > 0.01 (a ~0 dB peak/shelf is a no-op), but a LOWPASS band (type 4) is
            // gain-independent and must always run -- the old `gain > 0.01` gate silently skipped
            // every lowpass band (they carry 0 gain), so a type-4 EQ band did nothing.
            if self.enabled[b] && (self.gains_db[b].abs() > 0.01 || self.types[b] == 4) {
                l = self.bands_l[b].process(l);
                r = self.bands_r[b].process(r);
            }
        }
        (l, r)
    }

    pub fn reset(&mut self) {
        for b in 0..4 { self.bands_l[b].reset(); self.bands_r[b].reset(); }
    }

    pub fn set_band_freq(&mut self, b: usize, f: f32) {
        if b < 4 { self.freqs[b] = f.clamp(20.0, 20000.0); self.update_band(b); }
    }
    pub fn set_band_gain(&mut self, b: usize, g: f32) {
        if b < 4 { self.gains_db[b] = g.clamp(-18.0, 18.0); self.update_band(b); }
    }
    pub fn set_band_q(&mut self, b: usize, q: f32) {
        if b < 4 { self.qs[b] = q.clamp(0.1, 10.0); self.update_band(b); }
    }
    pub fn set_band_enabled(&mut self, b: usize, en: bool) {
        if b < 4 { self.enabled[b] = en; }
    }

    /// Evaluate the combined magnitude response at `freq`, in linear scale.
    /// Used by the GUI to draw the FR curve.
    pub fn magnitude_at(&self, freq: f32) -> f32 {
        let mut m = 1.0;
        for b in 0..4 {
            if self.enabled[b] {
                m *= self.bands_l[b].magnitude_at(self.sample_rate, freq);
            }
        }
        m
    }
    pub fn band_freq(&self, b: usize) -> f32 { self.freqs[b.min(3)] }
    pub fn band_gain(&self, b: usize) -> f32 { self.gains_db[b.min(3)] }
    pub fn band_q(&self, b: usize) -> f32 { self.qs[b.min(3)] }
    pub fn band_enabled(&self, b: usize) -> bool { self.enabled[b.min(3)] }
    pub fn band_type(&self, b: usize) -> u8 { self.types[b.min(3)] }
}

const TYPES: [&str; 5] = ["auto", "peak", "low-shelf", "high-shelf", "low-pass"];
const TYPE_LABELS: [&str; 5] = ["Auto", "Peak", "Low Shelf", "High Shelf", "Low Pass"];

macro_rules! band {
    ($b:literal, $name:literal, $fmin:literal, $fmax:literal, $fdef:literal, $gdef:literal, $qmax:literal) => {
        [
            ParamSpec { id: concat!("band.", $b, ".freq"), name: concat!($name, " Frequency"), short: concat!($name, " f"), kind: ParamKind::Float { min: $fmin, max: $fmax, curve: Curve::Log }, unit: Unit::Hz, default: Value::F($fdef), flags: ParamFlags::AUTOMATABLE },
            ParamSpec { id: concat!("band.", $b, ".gain"), name: concat!($name, " Gain"), short: concat!($name, " g"), kind: ParamKind::Float { min: -18.0, max: 18.0, curve: Curve::Linear }, unit: Unit::Db, default: Value::F($gdef), flags: ParamFlags::AUTOMATABLE },
            ParamSpec { id: concat!("band.", $b, ".q"), name: concat!($name, " Q"), short: concat!($name, " Q"), kind: ParamKind::Float { min: 0.3, max: $qmax, curve: Curve::Log }, unit: Unit::Q, default: Value::F(0.707), flags: ParamFlags::AUTOMATABLE },
            ParamSpec { id: concat!("band.", $b, ".enabled"), name: concat!($name, " On"), short: "On", kind: ParamKind::Bool, unit: Unit::None, default: Value::B(true), flags: ParamFlags::NONE },
            ParamSpec { id: concat!("band.", $b, ".type"), name: concat!($name, " Type"), short: "Type", kind: ParamKind::Enum { variants: &TYPES, labels: &TYPE_LABELS }, unit: Unit::None, default: Value::E("auto"), flags: ParamFlags::NONE },
        ]
    };
}

const B0: [ParamSpec; 5] = band!("0", "Low", 30.0, 500.0, 100.0, 0.0, 2.0);
const B1: [ParamSpec; 5] = band!("1", "Mid 1", 80.0, 2000.0, 500.0, 0.0, 10.0);
const B2: [ParamSpec; 5] = band!("2", "Mid 2", 500.0, 8000.0, 2000.0, 0.0, 10.0);
const B3: [ParamSpec; 5] = band!("3", "High", 2000.0, 18000.0, 8000.0, 0.0, 2.0);

pub static PARAMS: [ParamSpec; 20] = [
    B0[0], B0[1], B0[2], B0[3], B0[4],
    B1[0], B1[1], B1[2], B1[3], B1[4],
    B2[0], B2[1], B2[2], B2[3], B2[4],
    B3[0], B3[1], B3[2], B3[3], B3[4],
];

pub static SPEC: EffectSpec = EffectSpec {
    kind: "parametric-eq",
    name: "Parametric EQ",
    category: Category::Eq,
    params: &PARAMS,
    readouts: &[],
    needs: Needs::NONE,
};

pub fn build(sample_rate: f32) -> Box<dyn Effect> {
    Box::new(ParametricEq::new(sample_rate))
}

impl Effect for ParametricEq {
    fn spec(&self) -> &'static EffectSpec {
        &SPEC
    }

    fn set_param(&mut self, index: usize, value: Value) {
        let (b, what) = (index / 5, index % 5);
        match what {
            0 => self.set_band_freq(b, value.as_f32()),
            1 => self.set_band_gain(b, value.as_f32()),
            2 => self.set_band_q(b, value.as_f32()),
            3 => self.set_band_enabled(b, value.as_bool()),
            4 => self.set_band_type(b, PARAMS[index].variant_index(value.as_variant().unwrap_or("auto")).unwrap_or(0) as u8),
            _ => {}
        }
    }

    fn param(&self, index: usize) -> Value {
        let (b, what) = (index / 5, index % 5);
        match what {
            0 => Value::F(self.band_freq(b)),
            1 => Value::F(self.band_gain(b)),
            2 => Value::F(self.band_q(b)),
            3 => Value::B(self.band_enabled(b)),
            _ => Value::E(TYPES[(self.band_type(b) as usize).min(TYPES.len() - 1)]),
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
        ParametricEq::reset(self)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        *self = ParametricEq::new(sample_rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_is_sound_and_its_size_is_frozen() {
        assert!(SPEC.problems().is_empty(), "{:?}", SPEC.problems());
        assert_eq!(SPEC.params.len(), 20);
    }

    #[test]
    fn every_parameter_reads_back_what_was_set() {
        let mut eq = ParametricEq::new(48_000.0);
        for (i, p) in PARAMS.iter().enumerate() {
            let v = match p.kind {
                ParamKind::Float { min, max, .. } => Value::F((min + max) * 0.5),
                ParamKind::Bool => Value::B(false),
                ParamKind::Enum { variants, .. } => Value::E(variants[1]),
                _ => p.default,
            };
            eq.set_param(i, v);
            assert_eq!(eq.param(i), v, "{}", p.id);
        }
    }
}
