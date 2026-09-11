//! A bucket-brigade chorus: two delay lines a few milliseconds long,
//! modulated in quadrature so the two channels move against each other.
//!
//! The delay itself is `phonix_dsp::chorus::StereoChorus`, which is the
//! primitive every instrument that wants a chorus already shares. This wraps
//! it as an effect a chain can hold.

use crate::effect::{Effect, Ports, Value};
use crate::spec::{Category, Curve, EffectSpec, Needs, ParamFlags, ParamKind, ParamSpec, Unit};
use phonix_dsp::chorus::bbd_dimension::StereoChorus;

pub struct Chorus {
    inner: StereoChorus,
    /// Base delay in milliseconds: how far behind the dry the wet sits.
    delay_ms: f32,
    /// How far the delay travels, in milliseconds.
    depth_ms: f32,
    rate_hz: f32,
}

impl Chorus {
    pub fn new(sample_rate: f32) -> Self {
        let mut inner = StereoChorus::new(sample_rate);
        let (delay_ms, depth_ms, rate_hz) = (12.0, 3.0, 0.6);
        inner.set_params(delay_ms, depth_ms, rate_hz);
        Self { inner, delay_ms, depth_ms, rate_hz }
    }

    fn push(&mut self) {
        self.inner.set_params(self.delay_ms, self.depth_ms, self.rate_hz);
    }
}

pub static PARAMS: [ParamSpec; 3] = [
    ParamSpec { id: "rate", name: "Rate", short: "Rate", kind: ParamKind::Float { min: 0.02, max: 8.0, curve: Curve::Log }, unit: Unit::Hz, default: Value::F(0.6), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "depth", name: "Depth", short: "Depth", kind: ParamKind::Float { min: 0.0, max: 10.0, curve: Curve::Linear }, unit: Unit::Ms, default: Value::F(3.0), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "delay", name: "Delay", short: "Delay", kind: ParamKind::Float { min: 1.0, max: 40.0, curve: Curve::Linear }, unit: Unit::Ms, default: Value::F(12.0), flags: ParamFlags::AUTOMATABLE },
];

pub static SPEC: EffectSpec = EffectSpec {
    kind: "chorus",
    name: "Chorus",
    category: Category::Modulation,
    params: &PARAMS,
    readouts: &[],
    needs: Needs::NONE,
};

pub fn build(sample_rate: f32) -> Box<dyn Effect> {
    Box::new(Chorus::new(sample_rate))
}

impl Effect for Chorus {
    fn spec(&self) -> &'static EffectSpec {
        &SPEC
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.rate_hz = value.as_f32().clamp(0.02, 8.0),
            1 => self.depth_ms = value.as_f32().clamp(0.0, 10.0),
            2 => self.delay_ms = value.as_f32().clamp(1.0, 40.0),
            _ => return,
        }
        self.push();
    }

    fn param(&self, index: usize) -> Value {
        match index {
            0 => Value::F(self.rate_hz),
            1 => Value::F(self.depth_ms),
            _ => Value::F(self.delay_ms),
        }
    }

    /// Fully wet: the chain's own slot mix is what balances it against the
    /// dry, the way every other effect here works.
    fn process(&mut self, ports: &mut Ports<'_>) {
        for i in 0..ports.frames() {
            let (l, r) = self.inner.process(ports.audio.l[i], ports.audio.r[i], 1.0);
            ports.audio.l[i] = l;
            ports.audio.r[i] = r;
        }
    }

    fn reset(&mut self) {
        self.inner.reset_state();
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        let (delay_ms, depth_ms, rate_hz) = (self.delay_ms, self.depth_ms, self.rate_hz);
        *self = Chorus::new(sample_rate);
        self.delay_ms = delay_ms;
        self.depth_ms = depth_ms;
        self.rate_hz = rate_hz;
        self.push();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_is_sound_and_its_size_is_frozen() {
        assert!(SPEC.problems().is_empty(), "{:?}", SPEC.problems());
        assert_eq!(SPEC.params.len(), 3);
    }

    /// A chorus moves: the same input twice, a second apart, does not come
    /// back the same, because the delay has travelled.
    #[test]
    fn the_delay_travels() {
        let mut fx = Chorus::new(48_000.0);
        fx.set_param(0, Value::F(2.0));
        fx.set_param(1, Value::F(6.0));
        let run = |fx: &mut Chorus, n: usize| -> Vec<f32> {
            let mut l: Vec<f32> = (0..n).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
            let mut r = l.clone();
            let mut ports = Ports {
                audio: crate::effect::StereoMut { l: &mut l, r: &mut r },
                buses: &[], sidechain: None, modulator: None,
                transport: Default::default(), musical: Default::default(),
                sample_rate: 48_000.0,
            };
            fx.process(&mut ports);
            l
        };
        // Let the line fill, then take two windows a quarter of the LFO's
        // period apart: at 2 Hz that is 6000 samples.
        let _ = run(&mut fx, 24_000);
        let a = run(&mut fx, 6_000);
        let b = run(&mut fx, 6_000);
        let diff: f32 = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32;
        assert!(diff > 1e-3, "the chorus stood still: {diff}");
        for v in a.iter().chain(&b) {
            assert!(v.is_finite() && v.abs() < 4.0, "the chorus ran away to {v}");
        }
    }
}
