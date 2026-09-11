//! What an effect says about itself: its kind, its parameters, their ranges
//! and units, what it publishes, and what it needs from a host.
//!
//! The string id of a parameter and the string id of a kind are the only
//! wire formats in this crate. Indices are a convenience for the audio thread
//! and may move between minor versions; ids may not.

use crate::effect::Value;

/// A family, for a picker to group by. Not a wire format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Eq,
    Dynamics,
    Reverb,
    Delay,
    Modulation,
    Distortion,
    Spatial,
    Pitch,
    Meter,
    Utility,
    Rack,
}

/// What an effect reads from `Ports` beyond the audio, so a host can skip
/// gathering what no slot in the chain wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Needs(u8);

impl Needs {
    pub const NONE: Needs = Needs(0);
    pub const SIDECHAIN: Needs = Needs(1);
    pub const MODULATOR: Needs = Needs(2);
    pub const TEMPO: Needs = Needs(4);
    pub const KEY: Needs = Needs(8);
    pub const CHORD: Needs = Needs(16);

    pub const fn union(self, other: Needs) -> Needs {
        Needs(self.0 | other.0)
    }
    pub const fn contains(self, other: Needs) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Needs {
    type Output = Needs;
    fn bitor(self, rhs: Needs) -> Needs {
        self.union(rhs)
    }
}

/// How a parameter is displayed. Formatting lives here so no editor types a
/// unit at a knob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    None,
    Hz,
    Db,
    Ms,
    Seconds,
    /// A 0..1 value shown as 0..100.
    Percent,
    Ratio,
    Q,
    Semitones,
    Cents,
    Bpm,
}

impl Unit {
    pub fn format(self, v: f32) -> String {
        match self {
            Unit::None => format!("{v:.2}"),
            Unit::Hz => {
                if v >= 1000.0 { format!("{:.2} kHz", v / 1000.0) } else { format!("{v:.0} Hz") }
            }
            Unit::Db => format!("{v:+.1} dB"),
            Unit::Ms => format!("{v:.0} ms"),
            Unit::Seconds => {
                if v < 1.0 { format!("{:.0} ms", v * 1000.0) } else { format!("{v:.2} s") }
            }
            Unit::Percent => format!("{:.0}%", v * 100.0),
            Unit::Ratio => format!("{v:.1}:1"),
            Unit::Q => format!("{v:.2}"),
            Unit::Semitones => format!("{v:+.0} st"),
            Unit::Cents => format!("{v:+.0} ct"),
            Unit::Bpm => format!("{v:.1} bpm"),
        }
    }
}

/// How a float parameter's range is travelled by a control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    Linear,
    Log,
    Pow(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamKind {
    Float { min: f32, max: f32, curve: Curve },
    Int { min: i32, max: i32 },
    Bool,
    /// Named variants; `labels` is what a panel shows, one per variant.
    Enum { variants: &'static [&'static str], labels: &'static [&'static str] },
    /// `count` independent bits in one integer, for step masks that must
    /// round-trip an OFF step.
    Bits { count: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ParamFlags(u8);

impl ParamFlags {
    pub const NONE: ParamFlags = ParamFlags(0);
    pub const AUTOMATABLE: ParamFlags = ParamFlags(1);
    /// A momentary action, not a value to hold.
    pub const TRIGGER: ParamFlags = ParamFlags(2);
    pub const READ_ONLY: ParamFlags = ParamFlags(4);

    pub const fn union(self, other: ParamFlags) -> ParamFlags {
        ParamFlags(self.0 | other.0)
    }
    pub const fn contains(self, other: ParamFlags) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn automatable(self) -> bool {
        self.contains(ParamFlags::AUTOMATABLE)
    }
}

impl std::ops::BitOr for ParamFlags {
    type Output = ParamFlags;
    fn bitor(self, rhs: ParamFlags) -> ParamFlags {
        self.union(rhs)
    }
}

/// One parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamSpec {
    /// The wire format: `"band.0.freq"`, `"threshold"`, `"type"`.
    pub id: &'static str,
    pub name: &'static str,
    /// What fits under a knob.
    pub short: &'static str,
    pub kind: ParamKind,
    pub unit: Unit,
    pub default: Value,
    pub flags: ParamFlags,
}

impl ParamSpec {
    /// The index of a named enum variant, for a spec value given as text.
    pub fn variant_index(&self, name: &str) -> Option<usize> {
        match self.kind {
            ParamKind::Enum { variants, .. } => variants.iter().position(|v| *v == name),
            _ => None,
        }
    }

    /// The label of a named variant, or the name itself.
    pub fn variant_label(&self, name: &str) -> &'static str {
        match self.kind {
            ParamKind::Enum { variants, labels } => variants
                .iter()
                .position(|v| *v == name)
                .and_then(|i| labels.get(i).copied())
                .or_else(|| variants.iter().find(|v| **v == name).copied())
                .unwrap_or(""),
            _ => "",
        }
    }

    /// Whether `v` is a value this parameter can hold as it is.
    pub fn accepts(&self, v: Value) -> bool {
        match (self.kind, v) {
            (ParamKind::Float { min, max, .. }, Value::F(x)) => x >= min && x <= max,
            (ParamKind::Int { min, max }, Value::I(x)) => x >= min && x <= max,
            (ParamKind::Bool, Value::B(_)) => true,
            (ParamKind::Enum { variants, .. }, Value::E(name)) => variants.contains(&name),
            (ParamKind::Bits { count }, Value::I(x)) => x >= 0 && (x as u64) < (1u64 << count),
            _ => false,
        }
    }

    /// `v` brought inside the parameter's range, as the effect will hold it.
    pub fn clamp(&self, v: Value) -> Value {
        match (self.kind, v) {
            (ParamKind::Float { min, max, .. }, Value::F(x)) => Value::F(x.clamp(min, max)),
            (ParamKind::Float { min, max, .. }, other) => Value::F(other.as_f32().clamp(min, max)),
            (ParamKind::Int { min, max }, other) => Value::I(other.as_i32().clamp(min, max)),
            (ParamKind::Bool, other) => Value::B(other.as_bool()),
            (ParamKind::Enum { variants, .. }, Value::E(name)) if variants.contains(&name) => v,
            (ParamKind::Enum { .. }, _) => self.default,
            (ParamKind::Bits { count }, other) => {
                Value::I(other.as_i32().clamp(0, ((1u64 << count) - 1) as i32))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadoutKind {
    Scalar,
    Array(usize),
}

/// A value an effect publishes: a level, a gain reduction, a spectrum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadoutSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: ReadoutKind,
    pub unit: Unit,
}

/// One kind of effect, described.
#[derive(Debug, PartialEq)]
pub struct EffectSpec {
    /// The wire format: kebab-case, `"parametric-eq"`.
    pub kind: &'static str,
    pub name: &'static str,
    pub category: Category,
    /// Append-only within a minor version: the index of a parameter is what
    /// the audio thread uses.
    pub params: &'static [ParamSpec],
    pub readouts: &'static [ReadoutSpec],
    pub needs: Needs,
}

impl EffectSpec {
    pub fn param_index(&self, id: &str) -> Option<usize> {
        self.params.iter().position(|p| p.id == id)
    }

    pub fn param(&self, id: &str) -> Option<&ParamSpec> {
        self.params.iter().find(|p| p.id == id)
    }

    pub fn readout_index(&self, id: &str) -> Option<usize> {
        self.readouts.iter().position(|r| r.id == id)
    }

    pub fn automatable(&self) -> impl Iterator<Item = (usize, &ParamSpec)> {
        self.params.iter().enumerate().filter(|(_, p)| p.flags.automatable())
    }

    /// Everything a spec can get wrong, as text: duplicate ids, a default
    /// outside its range, a variant listed twice, a kind that is not
    /// kebab-case. Empty when the spec is sound. Every effect's test calls it.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !is_kebab(self.kind) {
            out.push(format!("kind {:?} is not kebab-case", self.kind));
        }
        for (i, p) in self.params.iter().enumerate() {
            if self.params[..i].iter().any(|q| q.id == p.id) {
                out.push(format!("parameter id {:?} is listed twice", p.id));
            }
            if !is_param_id(p.id) {
                out.push(format!("parameter id {:?} is not lower-case dotted", p.id));
            }
            if !p.accepts(p.default) {
                out.push(format!("parameter {:?} has a default outside its range", p.id));
            }
            if let ParamKind::Enum { variants, labels } = p.kind {
                for (j, v) in variants.iter().enumerate() {
                    if variants[..j].contains(v) {
                        out.push(format!("parameter {:?} lists variant {:?} twice", p.id, v));
                    }
                    if !is_kebab(v) {
                        out.push(format!("parameter {:?} has a variant {:?} that is not kebab-case", p.id, v));
                    }
                }
                if variants.is_empty() {
                    out.push(format!("parameter {:?} is an enum with no variant", p.id));
                }
                if labels.len() != variants.len() {
                    out.push(format!("parameter {:?} has {} labels for {} variants", p.id, labels.len(), variants.len()));
                }
            }
            if let ParamKind::Float { min, max, .. } = p.kind {
                if min >= max || min.is_nan() || max.is_nan() {
                    out.push(format!("parameter {:?} has an empty range", p.id));
                }
            }
        }
        for (i, r) in self.readouts.iter().enumerate() {
            if self.readouts[..i].iter().any(|q| q.id == r.id) {
                out.push(format!("readout id {:?} is listed twice", r.id));
            }
        }
        out
    }
}

fn is_kebab(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

fn is_param_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && !s.starts_with('.')
        && !s.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_format_the_way_a_panel_reads_them() {
        assert_eq!(Unit::Hz.format(90.0), "90 Hz");
        assert_eq!(Unit::Hz.format(1500.0), "1.50 kHz");
        assert_eq!(Unit::Db.format(-3.0), "-3.0 dB");
        assert_eq!(Unit::Percent.format(0.22), "22%");
        assert_eq!(Unit::Ratio.format(2.0), "2.0:1");
        assert_eq!(Unit::Seconds.format(0.02), "20 ms");
        assert_eq!(Unit::Seconds.format(1.5), "1.50 s");
    }

    #[test]
    fn problems_catch_what_a_hand_typed_table_gets_wrong() {
        static P: [ParamSpec; 2] = [
            ParamSpec { id: "gain", name: "Gain", short: "Gain", kind: ParamKind::Float { min: -18.0, max: 18.0, curve: Curve::Linear }, unit: Unit::Db, default: Value::F(40.0), flags: ParamFlags::AUTOMATABLE },
            ParamSpec { id: "gain", name: "Gain", short: "Gain", kind: ParamKind::Enum { variants: &["a", "a"], labels: &["A", "A"] }, unit: Unit::None, default: Value::E("a"), flags: ParamFlags::NONE },
        ];
        let spec = EffectSpec { kind: "Bad Kind", name: "x", category: Category::Utility, params: &P, readouts: &[], needs: Needs::NONE };
        let problems = spec.problems();
        assert!(problems.iter().any(|p| p.contains("kebab")));
        assert!(problems.iter().any(|p| p.contains("listed twice")));
        assert!(problems.iter().any(|p| p.contains("outside its range")));
        assert!(problems.iter().any(|p| p.contains("variant")));
    }

    #[test]
    fn clamping_lands_inside_the_range_and_enums_fall_back_to_the_default() {
        let p = ParamSpec { id: "type", name: "Type", short: "Type", kind: ParamKind::Enum { variants: &["room", "hall"], labels: &["Room", "Hall"] }, unit: Unit::None, default: Value::E("room"), flags: ParamFlags::NONE };
        assert_eq!(p.clamp(Value::E("hall")), Value::E("hall"));
        assert_eq!(p.clamp(Value::E("cave")), Value::E("room"));
        assert_eq!(p.variant_index("hall"), Some(1));
        let f = ParamSpec { id: "x", name: "x", short: "x", kind: ParamKind::Float { min: 0.0, max: 1.0, curve: Curve::Linear }, unit: Unit::None, default: Value::F(0.5), flags: ParamFlags::NONE };
        assert_eq!(f.clamp(Value::F(7.0)), Value::F(1.0));
    }
}
