//! Mid/side width with the bass held to the centre: the side below the
//! mono frequency is removed, the rest scaled by the width.

use crate::effect::{Effect, Ports, Value};
use crate::spec::{Category, Curve, EffectSpec, Needs, ParamFlags, ParamKind, ParamSpec, Unit};

pub struct StereoImager {
    sample_rate: f32,

    // Parameters
    width: f32,     // 0..2 (0=mono, 1=original, 2=extra wide)
    mono_freq: f32, // 20-500 Hz, frequencies below this are mono'd

    // Cached
    cached_mono_alpha: f32,

    // State (1-pole filters for bass mono)
    lp_state: f32, // LP on mid for bass extraction
    hp_state: f32, // HP on side for bass removal
}

impl StereoImager {
    pub fn new(sample_rate: f32) -> Self {
        let mono_freq = 100.0;
        let alpha = Self::compute_mono_alpha(mono_freq, sample_rate);
        Self {
            sample_rate,
            width: 1.0,
            mono_freq,
            cached_mono_alpha: alpha,
            lp_state: 0.0,
            hp_state: 0.0,
        }
    }

    #[inline(always)]
    fn compute_mono_alpha(freq: f32, sample_rate: f32) -> f32 {
        let rc = 1.0 / (2.0 * std::f32::consts::PI * freq);
        let dt = 1.0 / sample_rate;
        dt / (rc + dt)
    }

    fn tick(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {

        // Encode to mid/side
        let mid = (input_l + input_r) * 0.5;
        let side = (input_l - input_r) * 0.5;

        // Bass mono: LP filter on mid to extract bass, HP on side to remove bass
        let alpha = self.cached_mono_alpha;
        self.lp_state += alpha * (mid - self.lp_state);
        let _bass_mid = self.lp_state;

        // HP on side: side_hp = side - lp(side)
        self.hp_state += alpha * (side - self.hp_state);
        let side_hp = side - self.hp_state; // high-passed side (bass removed)

        // Reconstruct: bass stays in mid (mono), side only has HF content
        let final_mid = mid; // keep full mid
        let final_side = side_hp * self.width; // width-scaled side without bass

        // Add back the bass that was removed from side as mono (it's already in mid)
        // Decode back to L/R
        let wet_l = final_mid + final_side;
        let wet_r = final_mid - final_side;

        (wet_l, wet_r)
    }

    pub fn reset(&mut self) {
        self.lp_state = 0.0;
        self.hp_state = 0.0;
    }

    // Setters
    pub fn set_width(&mut self, w: f32) {
        self.width = w.clamp(0.0, 2.0);
    }
    pub fn set_mono_freq(&mut self, freq: f32) {
        self.mono_freq = freq.clamp(20.0, 500.0);
        self.cached_mono_alpha = Self::compute_mono_alpha(self.mono_freq, self.sample_rate);
    }
}

pub static PARAMS: [ParamSpec; 2] = [
    ParamSpec { id: "width", name: "Width", short: "Width", kind: ParamKind::Float { min: 0.0, max: 2.0, curve: Curve::Linear }, unit: Unit::Percent, default: Value::F(1.0), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "mono-freq", name: "Bass Mono Below", short: "Bass", kind: ParamKind::Float { min: 20.0, max: 500.0, curve: Curve::Log }, unit: Unit::Hz, default: Value::F(100.0), flags: ParamFlags::AUTOMATABLE },
];

pub static SPEC: EffectSpec = EffectSpec {
    kind: "stereo-imager",
    name: "Stereo Imager",
    category: Category::Spatial,
    params: &PARAMS,
    readouts: &[],
    needs: Needs::NONE,
};

pub fn build(sample_rate: f32) -> Box<dyn Effect> {
    Box::new(StereoImager::new(sample_rate))
}

impl Effect for StereoImager {
    fn spec(&self) -> &'static EffectSpec {
        &SPEC
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.set_width(value.as_f32()),
            1 => self.set_mono_freq(value.as_f32()),
            _ => {}
        }
    }

    fn param(&self, index: usize) -> Value {
        match index {
            0 => Value::F(self.width),
            _ => Value::F(self.mono_freq),
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
        StereoImager::reset(self)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        *self = StereoImager::new(sample_rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_is_sound_and_its_size_is_frozen() {
        assert!(SPEC.problems().is_empty(), "{:?}", SPEC.problems());
        assert_eq!(SPEC.params.len(), 2);
    }

    /// Width zero folds a wide signal to mono; width two doubles its side.
    #[test]
    fn the_width_scales_the_side_and_leaves_the_mid() {
        let side_of = |width: f32| -> f32 {
            let mut fx = StereoImager::new(48_000.0);
            fx.set_param(0, Value::F(width));
            let n = 4800;
            let mut l: Vec<f32> = (0..n).map(|i| (i as f32 * 0.2).sin() * 0.5).collect();
            let mut r: Vec<f32> = l.iter().map(|v| -v).collect();
            let mut ports = Ports { audio: crate::effect::StereoMut { l: &mut l, r: &mut r }, buses: &[], sidechain: None, modulator: None, transport: Default::default(), musical: Default::default(), sample_rate: 48_000.0 };
            fx.process(&mut ports);
            l[2400..].iter().zip(&r[2400..]).map(|(a, b)| ((a - b) * 0.5).abs()).fold(0.0_f32, f32::max)
        };
        assert!(side_of(0.0) < 1e-3, "{}", side_of(0.0));
        assert!((side_of(2.0) / side_of(1.0) - 2.0).abs() < 0.05, "{} vs {}", side_of(2.0), side_of(1.0));
    }
}
