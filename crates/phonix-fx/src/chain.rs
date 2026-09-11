//! Slots in series. Each slot holds one effect, its description, and a
//! delay line that keeps the dry path aligned with a wet path that reports
//! latency, so a partial mix never combs. The crossfade is equal-power:
//! `dry * cos(mix * pi/2) + wet * sin(mix * pi/2)`.
//!
//! A slot's output is soft-clipped beyond full scale and clamped to +-1.5,
//! and an effect whose output goes non-finite is reset and bypassed for
//! that block, so a chain of many slots cannot accumulate gain or poison a
//! render.
//!
//! `apply`, `prepare` and `set_sample_rate` allocate; `process` and the
//! live `set_param` do not.

use std::collections::BTreeMap;

use crate::chain_spec::{ChainSpec, Fit, SlotSpec, SpecValue};
use crate::effect::{Effect, Musical, Ports, Stereo, StereoMut, Transport, Value};
use crate::registry::Registry;
use crate::report::{ApplyReport, Clamped, Missing, Unknown};
use crate::spec::{Category, EffectSpec, Needs};

/// A live parameter, resolved from its id once at apply time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamRef {
    pub slot: usize,
    pub index: usize,
}

/// Stands in for a kind the build lacks: audio passes through, the
/// description is kept verbatim.
pub struct Placeholder;

pub static PLACEHOLDER_SPEC: EffectSpec = EffectSpec {
    kind: "missing",
    name: "Missing",
    category: Category::Utility,
    params: &[],
    readouts: &[],
    needs: Needs::NONE,
};

impl Effect for Placeholder {
    fn spec(&self) -> &'static EffectSpec {
        &PLACEHOLDER_SPEC
    }
    fn set_param(&mut self, _index: usize, _value: Value) {}
    fn param(&self, _index: usize) -> Value {
        Value::F(0.0)
    }
    fn process(&mut self, _ports: &mut Ports<'_>) {}
    fn reset(&mut self) {}
    fn set_sample_rate(&mut self, _sample_rate: f32) {}
}

/// Beyond full scale the curve bends to an asymptote at 2; the result is
/// clamped to +-1.5.
#[inline]
fn soft_clip(x: f32) -> f32 {
    let y = if x.abs() <= 1.0 { x } else { x.signum() * (2.0 - 1.0 / x.abs()) };
    y.clamp(-1.5, 1.5)
}

/// A ring buffer that delays one channel by a fixed count.
struct DryLine {
    buf: Vec<f32>,
    pos: usize,
}

impl DryLine {
    fn new(delay: usize) -> Self {
        DryLine { buf: vec![0.0; delay.max(1)], pos: 0 }
    }
    /// Delays `x` in place by `latency`, the line's length.
    fn run(&mut self, x: &mut [f32], latency: usize) {
        if latency == 0 {
            return;
        }
        for v in x.iter_mut() {
            std::mem::swap(&mut self.buf[self.pos], v);
            self.pos += 1;
            if self.pos == self.buf.len() {
                self.pos = 0;
            }
        }
    }
    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|v| *v = 0.0);
        self.pos = 0;
    }
}

struct Slot {
    effect: Box<dyn Effect>,
    /// The description as written, including parameters the kind does not
    /// declare and a placeholder's whole state.
    written: SlotSpec,
    placeholder: bool,
    enabled: bool,
    mix: f32,
    /// `cos` and `sin` of `mix * pi/2`.
    mix_dry: f32,
    mix_wet: f32,
    /// One value per declared parameter, kept current by `set_param` and
    /// re-applied after a sample-rate change.
    held: Vec<Value>,
    sidechain: Option<usize>,
    latency: usize,
    dry_l: DryLine,
    dry_r: DryLine,
    peak: f32,
}

impl Slot {
    fn set_mix(&mut self, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
        let angle = self.mix * std::f32::consts::FRAC_PI_2;
        self.mix_dry = angle.cos();
        self.mix_wet = angle.sin();
    }

    fn refresh_latency(&mut self) {
        let latency = if self.placeholder { 0 } else { self.effect.latency_samples() };
        if latency != self.latency {
            self.latency = latency;
            self.dry_l = DryLine::new(latency);
            self.dry_r = DryLine::new(latency);
        }
    }
}

pub struct Chain {
    sample_rate: f32,
    max_block: usize,
    slots: Vec<Slot>,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
}

impl Chain {
    /// An empty chain. `max_block` bounds one `process` call's work per
    /// pass; longer blocks are processed in pieces.
    pub fn new(sample_rate: f32, max_block: usize) -> Self {
        Chain {
            sample_rate,
            max_block: max_block.max(1),
            slots: Vec::new(),
            scratch_l: vec![0.0; max_block.max(1)],
            scratch_r: vec![0.0; max_block.max(1)],
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Resizes the scratch space and, when the rate changes, rebuilds every
    /// effect's state at the new rate with its parameters re-applied.
    pub fn prepare(&mut self, sample_rate: f32, max_block: usize) {
        self.max_block = max_block.max(1);
        self.scratch_l.resize(self.max_block, 0.0);
        self.scratch_r.resize(self.max_block, 0.0);
        if sample_rate != self.sample_rate {
            self.set_sample_rate(sample_rate);
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        for slot in &mut self.slots {
            slot.effect.set_sample_rate(sample_rate);
            for (index, value) in slot.held.iter().enumerate() {
                slot.effect.set_param(index, *value);
            }
            slot.refresh_latency();
            slot.dry_l.clear();
            slot.dry_r.clear();
        }
    }

    /// Rebuilds the chain from `spec`. Kinds `registry` lacks become
    /// placeholders; parameters the kind does not declare are kept in the
    /// description; values outside their range are clamped. All of it is
    /// reported.
    pub fn apply(&mut self, spec: &ChainSpec, registry: &Registry) -> ApplyReport {
        let mut report = ApplyReport::default();
        self.slots.clear();
        for (i, written) in spec.slots.iter().enumerate() {
            let (effect, placeholder) = match registry.build(&written.kind, self.sample_rate) {
                Some(e) => (e, false),
                None => {
                    report.missing.push(Missing { slot: i, kind: written.kind.clone() });
                    (Box::new(Placeholder) as Box<dyn Effect>, true)
                }
            };
            let mut slot = Slot {
                effect,
                written: written.clone(),
                placeholder,
                enabled: written.enabled,
                mix: 1.0,
                mix_dry: 0.0,
                mix_wet: 1.0,
                held: Vec::new(),
                sidechain: None,
                latency: 0,
                dry_l: DryLine::new(0),
                dry_r: DryLine::new(0),
                peak: 0.0,
            };
            slot.set_mix(written.mix);
            if !placeholder {
                let espec = slot.effect.spec();
                slot.held = espec.params.iter().map(|p| p.default).collect();
                for (id, value) in &written.params {
                    match espec.param_index(id) {
                        Some(index) => {
                            let (held, fit) = value.resolve(&espec.params[index]);
                            if fit == Fit::Clamped {
                                report.clamped.push(Clamped { slot: i, id: id.clone() });
                            }
                            slot.held[index] = held;
                        }
                        None => report.unknown.push(Unknown { slot: i, id: id.clone() }),
                    }
                }
                for (index, value) in slot.held.iter().enumerate() {
                    slot.effect.set_param(index, *value);
                }
                slot.refresh_latency();
            }
            self.slots.push(slot);
        }
        report
    }

    /// The chain as it stands: every declared parameter at its current
    /// value, undeclared ones as written, placeholders verbatim.
    pub fn spec(&self) -> ChainSpec {
        let slots = self
            .slots
            .iter()
            .map(|slot| {
                let mut out = slot.written.clone();
                out.enabled = slot.enabled;
                out.mix = slot.mix;
                if !slot.placeholder {
                    let espec = slot.effect.spec();
                    let mut params: BTreeMap<String, SpecValue> = slot
                        .written
                        .params
                        .iter()
                        .filter(|(id, _)| espec.param_index(id).is_none())
                        .map(|(id, v)| (id.clone(), v.clone()))
                        .collect();
                    for (index, p) in espec.params.iter().enumerate() {
                        params.insert(p.id.to_string(), SpecValue::from(slot.effect.param(index)));
                    }
                    out.params = params;
                }
                out
            })
            .collect();
        ChainSpec::new(slots)
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The description of `slot`'s kind; the placeholder's for a missing kind.
    pub fn effect_spec(&self, slot: usize) -> Option<&'static EffectSpec> {
        self.slots.get(slot).map(|s| s.effect.spec())
    }

    /// The kind `slot` was written with, whether or not the build has it.
    pub fn kind(&self, slot: usize) -> Option<&str> {
        self.slots.get(slot).map(|s| s.written.kind.as_str())
    }

    pub fn is_placeholder(&self, slot: usize) -> bool {
        self.slots.get(slot).is_some_and(|s| s.placeholder)
    }

    pub fn param_ref(&self, slot: usize, id: &str) -> Option<ParamRef> {
        let index = self.slots.get(slot)?.effect.spec().param_index(id)?;
        Some(ParamRef { slot, index })
    }

    /// Live control: clamps to the parameter's range, no allocation. A
    /// latency change the value causes takes effect at the next `apply`.
    pub fn set_param(&mut self, r: ParamRef, value: Value) {
        let Some(slot) = self.slots.get_mut(r.slot) else { return };
        let Some(p) = slot.effect.spec().params.get(r.index) else { return };
        let held = p.clamp(value);
        slot.held[r.index] = held;
        slot.effect.set_param(r.index, held);
    }

    pub fn param(&self, r: ParamRef) -> Option<Value> {
        let slot = self.slots.get(r.slot)?;
        slot.effect.spec().params.get(r.index)?;
        Some(slot.effect.param(r.index))
    }

    pub fn enabled(&self, slot: usize) -> bool {
        self.slots.get(slot).is_some_and(|s| s.enabled)
    }

    pub fn set_enabled(&mut self, slot: usize, enabled: bool) {
        if let Some(s) = self.slots.get_mut(slot) {
            s.enabled = enabled;
        }
    }

    pub fn mix(&self, slot: usize) -> f32 {
        self.slots.get(slot).map_or(1.0, |s| s.mix)
    }

    pub fn set_mix(&mut self, slot: usize, mix: f32) {
        if let Some(s) = self.slots.get_mut(slot) {
            s.set_mix(mix);
        }
    }

    /// The bus `slot` listens to, by index into `Ports::buses`. The host
    /// resolves the written `sidechain` name to this.
    pub fn set_sidechain(&mut self, slot: usize, bus: Option<usize>) {
        if let Some(s) = self.slots.get_mut(slot) {
            s.sidechain = bus;
        }
    }

    pub fn sidechain_name(&self, slot: usize) -> Option<&str> {
        self.slots.get(slot)?.written.sidechain.as_deref()
    }

    pub fn readout(&self, slot: usize, index: usize) -> f32 {
        self.slots.get(slot).map_or(f32::NAN, |s| s.effect.readout(index))
    }

    pub fn readout_array(&self, slot: usize, index: usize, out: &mut [f32]) -> usize {
        self.slots.get(slot).map_or(0, |s| s.effect.readout_array(index, out))
    }

    /// The absolute peak each slot produced in the last block, in order.
    pub fn slot_peaks(&self) -> impl Iterator<Item = f32> + '_ {
        self.slots.iter().map(|s| s.peak)
    }

    /// Latency of the enabled slots in series.
    pub fn latency_samples(&self) -> usize {
        self.slots.iter().filter(|s| s.enabled).map(|s| s.latency).sum()
    }

    /// What the enabled slots read beyond the audio.
    pub fn needs(&self) -> Needs {
        self.slots
            .iter()
            .filter(|s| s.enabled)
            .fold(Needs::NONE, |acc, s| acc.union(s.effect.spec().needs))
    }

    /// Whether a block would leave the audio untouched.
    pub fn is_idle(&self) -> bool {
        self.slots.iter().all(|s| !s.enabled || s.placeholder || s.mix == 0.0)
    }

    pub fn reset(&mut self) {
        for slot in &mut self.slots {
            slot.effect.reset();
            slot.dry_l.clear();
            slot.dry_r.clear();
            slot.peak = 0.0;
        }
    }

    /// One block through every enabled slot. `buses` are indexed by the
    /// slots' sidechain settings.
    pub fn process(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        buses: &[Stereo<'_>],
        transport: Transport,
        musical: Musical,
    ) {
        let frames = l.len().min(r.len());
        let mut start = 0;
        while start < frames {
            let end = (start + self.max_block).min(frames);
            self.process_piece(&mut l[start..end], &mut r[start..end], buses, transport, musical, start);
            start = end;
        }
    }

    fn process_piece(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        buses: &[Stereo<'_>],
        transport: Transport,
        musical: Musical,
        offset: usize,
    ) {
        let n = l.len();
        let sample_rate = self.sample_rate;
        for slot in &mut self.slots {
            if !slot.enabled || slot.placeholder || slot.mix == 0.0 {
                if offset == 0 {
                    slot.peak = 0.0;
                }
                continue;
            }
            let wet_only = slot.mix >= 1.0;
            let dry_kept = !wet_only || slot.latency == 0;
            if dry_kept {
                self.scratch_l[..n].copy_from_slice(l);
                self.scratch_r[..n].copy_from_slice(r);
            }
            let mut ports = Ports {
                audio: StereoMut { l, r },
                buses,
                sidechain: slot.sidechain,
                modulator: None,
                transport,
                musical,
                sample_rate,
            };
            slot.effect.process(&mut ports);
            let finite = l[..n].iter().chain(r[..n].iter()).all(|v| v.is_finite());
            if !finite {
                slot.effect.reset();
                if dry_kept {
                    l.copy_from_slice(&self.scratch_l[..n]);
                    r.copy_from_slice(&self.scratch_r[..n]);
                } else {
                    l.iter_mut().for_each(|v| *v = 0.0);
                    r.iter_mut().for_each(|v| *v = 0.0);
                }
            } else if !wet_only {
                let latency = slot.latency;
                slot.dry_l.run(&mut self.scratch_l[..n], latency);
                slot.dry_r.run(&mut self.scratch_r[..n], latency);
                let (wet, dry) = (slot.mix_wet, slot.mix_dry);
                for i in 0..n {
                    l[i] = soft_clip(self.scratch_l[i] * dry + l[i] * wet);
                    r[i] = soft_clip(self.scratch_r[i] * dry + r[i] * wet);
                }
            } else {
                for i in 0..n {
                    l[i] = soft_clip(l[i]);
                    r[i] = soft_clip(r[i]);
                }
            }
            let mut peak = if offset == 0 { 0.0 } else { slot.peak };
            for i in 0..n {
                peak = peak.max(l[i].abs()).max(r[i].abs());
            }
            slot.peak = peak;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Entry;
    use crate::spec::{Curve, ParamFlags, ParamKind, ParamSpec, Unit};

    /// Multiplies by a gain; declares one float and one enum.
    struct Gain {
        gain: f32,
        law: &'static str,
    }

    static GAIN_PARAMS: [ParamSpec; 2] = [
        ParamSpec { id: "gain", name: "Gain", short: "Gain", kind: ParamKind::Float { min: 0.0, max: 2.0, curve: Curve::Linear }, unit: Unit::None, default: Value::F(1.0), flags: ParamFlags::AUTOMATABLE },
        ParamSpec { id: "law", name: "Law", short: "Law", kind: ParamKind::Enum { variants: &["linear", "square"], labels: &["Linear", "Square"] }, unit: Unit::None, default: Value::E("linear"), flags: ParamFlags::NONE },
    ];
    static GAIN_SPEC: EffectSpec = EffectSpec { kind: "test-gain", name: "Gain", category: Category::Utility, params: &GAIN_PARAMS, readouts: &[], needs: Needs::NONE };

    impl Effect for Gain {
        fn spec(&self) -> &'static EffectSpec {
            &GAIN_SPEC
        }
        fn set_param(&mut self, index: usize, value: Value) {
            match index {
                0 => self.gain = value.as_f32(),
                1 => self.law = value.as_variant().unwrap_or("linear"),
                _ => {}
            }
        }
        fn param(&self, index: usize) -> Value {
            match index {
                0 => Value::F(self.gain),
                _ => Value::E(self.law),
            }
        }
        fn process(&mut self, ports: &mut Ports<'_>) {
            let g = if self.law == "square" { self.gain * self.gain } else { self.gain };
            for v in ports.audio.l.iter_mut().chain(ports.audio.r.iter_mut()) {
                *v *= g;
            }
        }
        fn reset(&mut self) {}
        fn set_sample_rate(&mut self, _sample_rate: f32) {
            *self = Gain { gain: f32::NAN, law: "" };
        }
    }

    /// Delays by `latency` samples and says so.
    struct Delay {
        latency: usize,
        line_l: Vec<f32>,
        line_r: Vec<f32>,
        pos: usize,
    }

    static DELAY_PARAMS: [ParamSpec; 1] = [ParamSpec { id: "latency", name: "Latency", short: "Lat", kind: ParamKind::Int { min: 0, max: 64 }, unit: Unit::None, default: Value::I(4), flags: ParamFlags::NONE }];
    static DELAY_SPEC: EffectSpec = EffectSpec { kind: "test-delay", name: "Delay", category: Category::Delay, params: &DELAY_PARAMS, readouts: &[], needs: Needs::SIDECHAIN };

    impl Effect for Delay {
        fn spec(&self) -> &'static EffectSpec {
            &DELAY_SPEC
        }
        fn set_param(&mut self, _index: usize, value: Value) {
            self.latency = value.as_i32() as usize;
            self.line_l = vec![0.0; self.latency.max(1)];
            self.line_r = vec![0.0; self.latency.max(1)];
            self.pos = 0;
        }
        fn param(&self, _index: usize) -> Value {
            Value::I(self.latency as i32)
        }
        fn process(&mut self, ports: &mut Ports<'_>) {
            if self.latency == 0 {
                return;
            }
            for i in 0..ports.frames() {
                let (ol, or) = (self.line_l[self.pos], self.line_r[self.pos]);
                self.line_l[self.pos] = ports.audio.l[i];
                self.line_r[self.pos] = ports.audio.r[i];
                ports.audio.l[i] = ol;
                ports.audio.r[i] = or;
                self.pos = (self.pos + 1) % self.latency;
            }
        }
        fn reset(&mut self) {
            self.line_l.iter_mut().for_each(|v| *v = 0.0);
            self.line_r.iter_mut().for_each(|v| *v = 0.0);
            self.pos = 0;
        }
        fn set_sample_rate(&mut self, _sample_rate: f32) {}
        fn latency_samples(&self) -> usize {
            self.latency
        }
    }

    fn registry() -> Registry {
        let mut r = Registry::new();
        r.register(Entry { spec: &GAIN_SPEC, build: |_| Box::new(Gain { gain: 1.0, law: "linear" }) });
        r.register(Entry { spec: &DELAY_SPEC, build: |_| Box::new(Delay { latency: 4, line_l: vec![0.0; 4], line_r: vec![0.0; 4], pos: 0 }) });
        r
    }

    fn run(chain: &mut Chain, l: &mut [f32], r: &mut [f32]) {
        chain.process(l, r, &[], Transport::default(), Musical::default());
    }

    #[test]
    fn ids_resolve_to_indices_once_and_values_land_where_written() {
        let spec = ChainSpec::new(vec![SlotSpec::new("test-gain").with("gain", 0.5_f32).with("law", "square")]);
        let mut chain = Chain::new(48_000.0, 64);
        let report = chain.apply(&spec, &registry());
        assert!(report.is_clean(), "{report}");
        let gain = chain.param_ref(0, "gain").unwrap();
        assert_eq!(gain, ParamRef { slot: 0, index: 0 });
        assert_eq!(chain.param(gain), Some(Value::F(0.5)));
        assert_eq!(chain.param(chain.param_ref(0, "law").unwrap()), Some(Value::E("square")));
        assert!(chain.param_ref(0, "nope").is_none());
        let mut l = [1.0_f32; 8];
        let mut r = [1.0_f32; 8];
        run(&mut chain, &mut l, &mut r);
        assert!(l.iter().all(|v| (*v - 0.25).abs() < 1e-7), "{l:?}");
    }

    #[test]
    fn apply_then_spec_is_the_identity_on_a_full_description() {
        let spec = ChainSpec::new(vec![
            SlotSpec::new("test-gain").with("gain", 0.75_f32).with("law", "square").mix(0.4),
            SlotSpec::new("test-delay").with("latency", 8_i64).enabled(false),
        ]);
        let mut chain = Chain::new(48_000.0, 64);
        assert!(chain.apply(&spec, &registry()).is_clean());
        assert_eq!(chain.spec(), spec);
    }

    #[test]
    fn a_partial_description_comes_back_completed_with_defaults() {
        let spec = ChainSpec::new(vec![SlotSpec::new("test-gain").with("gain", 0.75_f32)]);
        let mut chain = Chain::new(48_000.0, 64);
        assert!(chain.apply(&spec, &registry()).is_clean());
        let full = chain.spec();
        assert_eq!(full.slots[0].variant("law"), Some("linear"));
        assert_eq!(full.slots[0].f32("gain"), Some(0.75));
    }

    #[test]
    fn a_kind_the_build_lacks_passes_through_and_round_trips_verbatim() {
        let mut spec = ChainSpec::new(vec![
            SlotSpec::new("test-gain").with("gain", 0.5_f32).with("law", "linear"),
            SlotSpec::new("cinema-plate").with("size", 0.9_f32).with("type", "cathedral").mix(0.3),
            SlotSpec::new("test-gain").with("gain", 0.5_f32).with("law", "linear"),
        ]);
        spec.slots[1].sidechain = Some("bus.2".into());
        let mut chain = Chain::new(48_000.0, 64);
        let report = chain.apply(&spec, &registry());
        assert_eq!(report.missing, vec![Missing { slot: 1, kind: "cinema-plate".into() }]);
        assert!(report.unknown.is_empty() && report.clamped.is_empty());
        assert!(chain.is_placeholder(1));
        assert_eq!(chain.kind(1), Some("cinema-plate"));
        assert_eq!(chain.sidechain_name(1), Some("bus.2"));
        let mut l = [1.0_f32; 4];
        let mut r = [1.0_f32; 4];
        run(&mut chain, &mut l, &mut r);
        assert!(l.iter().all(|v| (*v - 0.25).abs() < 1e-7), "the placeholder is a wire: {l:?}");
        assert_eq!(chain.spec(), spec);
    }

    #[test]
    fn an_undeclared_parameter_is_reported_kept_and_ignored() {
        let spec = ChainSpec::new(vec![SlotSpec::new("test-gain").with("gain", 0.5_f32).with("shimmer", 1_i64)]);
        let mut chain = Chain::new(48_000.0, 64);
        let report = chain.apply(&spec, &registry());
        assert_eq!(report.unknown, vec![Unknown { slot: 0, id: "shimmer".into() }]);
        assert_eq!(chain.spec().slots[0].get("shimmer"), Some(&SpecValue::I(1)));
    }

    #[test]
    fn a_value_outside_its_range_is_clamped_and_reported() {
        let spec = ChainSpec::new(vec![SlotSpec::new("test-gain").with("gain", 9.0_f32).with("law", "cubic")]);
        let mut chain = Chain::new(48_000.0, 64);
        let report = chain.apply(&spec, &registry());
        assert_eq!(report.clamped.len(), 2, "{report}");
        assert_eq!(chain.param(ParamRef { slot: 0, index: 0 }), Some(Value::F(2.0)));
        assert_eq!(chain.param(ParamRef { slot: 0, index: 1 }), Some(Value::E("linear")));
    }

    #[test]
    fn a_half_wet_slot_with_latency_does_not_comb() {
        let spec = ChainSpec::new(vec![SlotSpec::new("test-delay").with("latency", 8_i64).mix(0.5)]);
        let mut chain = Chain::new(48_000.0, 16);
        assert!(chain.apply(&spec, &registry()).is_clean());
        assert_eq!(chain.latency_samples(), 8);
        let mut l = [0.0_f32; 32];
        let mut r = [0.0_f32; 32];
        l[0] = 0.5;
        r[0] = 0.5;
        run(&mut chain, &mut l, &mut r);
        let hits: Vec<usize> = l.iter().enumerate().filter(|(_, v)| **v != 0.0).map(|(i, _)| i).collect();
        assert_eq!(hits, vec![8], "one impulse, delayed, not two halves: {l:?}");
        let a = 0.5 * std::f32::consts::FRAC_PI_2;
        let want = 0.5 * (a.cos() + a.sin());
        assert!((l[8] - want).abs() < 1e-6, "{} vs {want}", l[8]);
        assert!((r[8] - want).abs() < 1e-6);
    }

    #[test]
    fn a_block_longer_than_the_chain_was_prepared_for_is_processed_whole() {
        let spec = ChainSpec::new(vec![SlotSpec::new("test-delay").with("latency", 3_i64)]);
        let mut chain = Chain::new(48_000.0, 4);
        chain.apply(&spec, &registry());
        let mut l: Vec<f32> = (0..19).map(|i| i as f32 * 0.05).collect();
        let mut r = l.clone();
        run(&mut chain, &mut l, &mut r);
        let expect: Vec<f32> = (0..19).map(|i| if i < 3 { 0.0 } else { (i - 3) as f32 * 0.05 }).collect();
        assert_eq!(l, expect);
        assert_eq!(r, expect);
    }

    #[test]
    fn a_sample_rate_change_keeps_every_parameter_including_live_edits() {
        let spec = ChainSpec::new(vec![SlotSpec::new("test-gain").with("gain", 0.5_f32).with("law", "square")]);
        let mut chain = Chain::new(48_000.0, 64);
        chain.apply(&spec, &registry());
        let gain = chain.param_ref(0, "gain").unwrap();
        chain.set_param(gain, Value::F(0.25));
        chain.set_sample_rate(96_000.0);
        assert_eq!(chain.param(gain), Some(Value::F(0.25)));
        assert_eq!(chain.param(ParamRef { slot: 0, index: 1 }), Some(Value::E("square")));
        assert_eq!(chain.sample_rate(), 96_000.0);
    }

    #[test]
    fn a_slot_that_goes_past_full_scale_is_bent_and_one_that_goes_non_finite_is_bypassed() {
        let spec = ChainSpec::new(vec![SlotSpec::new("test-gain").with("gain", 2.0_f32)]);
        let mut chain = Chain::new(48_000.0, 64);
        chain.apply(&spec, &registry());
        let mut l = [0.9_f32; 4];
        let mut r = [0.9_f32; 4];
        run(&mut chain, &mut l, &mut r);
        assert!((l[0] - (2.0 - 1.0 / 1.8)).abs() < 1e-6, "{}", l[0]);
        chain.set_param(ParamRef { slot: 0, index: 0 }, Value::F(f32::NAN));
        let mut l = [0.25_f32; 4];
        let mut r = [0.5_f32; 4];
        run(&mut chain, &mut l, &mut r);
        assert_eq!(l, [0.25; 4], "a slot that blew up is bypassed for the block");
        assert_eq!(r, [0.5; 4]);
    }

    #[test]
    fn live_edits_clamp_and_the_chain_reports_its_needs_and_peaks() {
        let spec = ChainSpec::new(vec![SlotSpec::new("test-gain"), SlotSpec::new("test-delay").with("latency", 0_i64)]);
        let mut chain = Chain::new(48_000.0, 64);
        chain.apply(&spec, &registry());
        assert_eq!(chain.needs(), Needs::SIDECHAIN);
        chain.set_enabled(1, false);
        assert_eq!(chain.needs(), Needs::NONE);
        chain.set_param(ParamRef { slot: 0, index: 0 }, Value::F(-3.0));
        assert_eq!(chain.param(ParamRef { slot: 0, index: 0 }), Some(Value::F(0.0)));
        chain.set_param(ParamRef { slot: 0, index: 0 }, Value::F(2.0));
        let mut l = [0.5_f32; 8];
        let mut r = [-0.25_f32; 8];
        run(&mut chain, &mut l, &mut r);
        let peaks: Vec<f32> = chain.slot_peaks().collect();
        assert_eq!(peaks, vec![1.0, 0.0]);
        assert!(!chain.is_idle());
        chain.set_mix(0, 0.0);
        assert!(chain.is_idle());
    }
}
