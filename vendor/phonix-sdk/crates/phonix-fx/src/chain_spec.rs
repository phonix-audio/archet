//! A chain described without a running effect: what a preset, a patch and a
//! session store. Kinds and parameters are named; unknown names survive a
//! round trip untouched.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::effect::Value;
use crate::spec::{ParamKind, ParamSpec};

/// The shape of `ChainSpec` this crate writes.
pub const VERSION: u32 = 1;

/// A parameter as written in a file. Integers are read before floats, so
/// `1` is `I(1)` and `1.0` is `F(1.0)`; an effect converts either to what
/// its parameter holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SpecValue {
    B(bool),
    I(i64),
    F(f32),
    /// An enum variant by name.
    S(String),
}

impl From<Value> for SpecValue {
    fn from(v: Value) -> Self {
        match v {
            Value::F(x) => SpecValue::F(x),
            Value::I(x) => SpecValue::I(x as i64),
            Value::B(b) => SpecValue::B(b),
            Value::E(name) => SpecValue::S(name.to_string()),
        }
    }
}

impl From<f32> for SpecValue {
    fn from(v: f32) -> Self {
        SpecValue::F(v)
    }
}
impl From<i64> for SpecValue {
    fn from(v: i64) -> Self {
        SpecValue::I(v)
    }
}
impl From<bool> for SpecValue {
    fn from(v: bool) -> Self {
        SpecValue::B(v)
    }
}
impl From<&str> for SpecValue {
    fn from(v: &str) -> Self {
        SpecValue::S(v.to_string())
    }
}

/// How a written value became what the effect holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Held as written.
    Exact,
    /// Brought inside the range, or an unknown variant replaced by the
    /// default.
    Clamped,
}

impl SpecValue {
    /// The value `param` holds for this written value.
    pub fn resolve(&self, param: &ParamSpec) -> (Value, Fit) {
        let wanted = match (param.kind, self) {
            (ParamKind::Enum { variants, .. }, SpecValue::S(name)) => {
                match variants.iter().find(|v| **v == name.as_str()) {
                    Some(v) => Value::E(v),
                    None => return (param.default, Fit::Clamped),
                }
            }
            (ParamKind::Enum { .. }, _) => return (param.default, Fit::Clamped),
            (_, SpecValue::S(_)) => return (param.default, Fit::Clamped),
            (_, SpecValue::F(x)) => Value::F(*x),
            (_, SpecValue::I(x)) => Value::I((*x).clamp(i32::MIN as i64, i32::MAX as i64) as i32),
            (_, SpecValue::B(b)) => Value::B(*b),
        };
        let held = param.clamp(wanted);
        let fit = if held.as_f32() == wanted.as_f32() && held.as_variant() == wanted.as_variant() {
            Fit::Exact
        } else {
            Fit::Clamped
        };
        (held, fit)
    }
}

/// One macro of a rack: a name and the parameters it moves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct MacroSpec {
    pub name: String,
    /// `0..1`.
    pub value: f32,
    pub targets: Vec<MacroTarget>,
}

/// One parameter a macro moves: `(chain, slot, id)` inside the rack, and
/// the range the macro's `0..1` maps onto.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MacroTarget {
    pub chain: usize,
    pub slot: usize,
    pub id: String,
    pub min: f32,
    pub max: f32,
}

/// One slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlotSpec {
    pub kind: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "one")]
    pub mix: f32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, SpecValue>,
    /// A bus name the host resolves; opaque here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidechain: Option<String>,
    /// Rack only: the parallel chains.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chains: Vec<ChainSpec>,
    /// Rack only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macros: Vec<MacroSpec>,
}

fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}

impl SlotSpec {
    pub fn new(kind: impl Into<String>) -> Self {
        SlotSpec {
            kind: kind.into(),
            enabled: true,
            mix: 1.0,
            params: BTreeMap::new(),
            sidechain: None,
            chains: Vec::new(),
            macros: Vec::new(),
        }
    }

    pub fn with(mut self, id: impl Into<String>, value: impl Into<SpecValue>) -> Self {
        self.params.insert(id.into(), value.into());
        self
    }

    pub fn mix(mut self, mix: f32) -> Self {
        self.mix = mix;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn set(&mut self, id: impl Into<String>, value: impl Into<SpecValue>) {
        self.params.insert(id.into(), value.into());
    }

    pub fn get(&self, id: &str) -> Option<&SpecValue> {
        self.params.get(id)
    }

    pub fn f32(&self, id: &str) -> Option<f32> {
        match self.params.get(id)? {
            SpecValue::F(x) => Some(*x),
            SpecValue::I(x) => Some(*x as f32),
            SpecValue::B(b) => Some(if *b { 1.0 } else { 0.0 }),
            SpecValue::S(_) => None,
        }
    }

    pub fn variant(&self, id: &str) -> Option<&str> {
        match self.params.get(id)? {
            SpecValue::S(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

/// A chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainSpec {
    #[serde(default = "version")]
    pub version: u32,
    #[serde(default)]
    pub slots: Vec<SlotSpec>,
}

fn version() -> u32 {
    VERSION
}

impl Default for ChainSpec {
    fn default() -> Self {
        ChainSpec { version: VERSION, slots: Vec::new() }
    }
}

impl ChainSpec {
    pub fn new(slots: Vec<SlotSpec>) -> Self {
        ChainSpec { version: VERSION, slots }
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// The kinds named, in order, with repeats.
    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        self.slots.iter().map(|s| s.kind.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{Curve, ParamFlags, Unit};

    #[test]
    fn a_chain_round_trips_through_json_with_names_only() {
        let spec = ChainSpec::new(vec![
            SlotSpec::new("reverb").with("type", "room").with("size", 0.3_f32).mix(0.22),
            SlotSpec::new("compressor").with("mode", "bus").with("ratio", 2.0_f32).with("bypass", false),
        ]);
        let text = serde_json::to_string_pretty(&spec).unwrap();
        assert!(text.contains("\"room\"") && text.contains("\"size\": 0.3"), "{text}");
        assert!(!text.contains("sidechain") && !text.contains("chains"), "{text}");
        let back: ChainSpec = serde_json::from_str(&text).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn a_parameter_nobody_declares_survives_a_round_trip() {
        let text = r#"{"slots":[{"kind":"reverb","params":{"from-the-future":7}}]}"#;
        let spec: ChainSpec = serde_json::from_str(text).unwrap();
        assert_eq!(spec.version, VERSION);
        assert_eq!(spec.slots[0].get("from-the-future"), Some(&SpecValue::I(7)));
        assert!(spec.slots[0].enabled);
        assert_eq!(spec.slots[0].mix, 1.0);
        let back: ChainSpec = serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn a_written_value_resolves_to_what_the_parameter_holds() {
        let f = ParamSpec { id: "size", name: "Size", short: "Size", kind: ParamKind::Float { min: 0.0, max: 1.0, curve: Curve::Linear }, unit: Unit::Percent, default: Value::F(0.5), flags: ParamFlags::NONE };
        assert_eq!(SpecValue::F(0.3).resolve(&f), (Value::F(0.3), Fit::Exact));
        assert_eq!(SpecValue::I(1).resolve(&f), (Value::F(1.0), Fit::Exact));
        assert_eq!(SpecValue::F(4.0).resolve(&f), (Value::F(1.0), Fit::Clamped));
        assert_eq!(SpecValue::S("big".into()).resolve(&f), (Value::F(0.5), Fit::Clamped));
        let e = ParamSpec { id: "type", name: "Type", short: "Type", kind: ParamKind::Enum { variants: &["room", "hall"], labels: &["Room", "Hall"] }, unit: Unit::None, default: Value::E("room"), flags: ParamFlags::NONE };
        assert_eq!(SpecValue::S("hall".into()).resolve(&e), (Value::E("hall"), Fit::Exact));
        assert_eq!(SpecValue::S("cave".into()).resolve(&e), (Value::E("room"), Fit::Clamped));
        assert_eq!(SpecValue::I(1).resolve(&e), (Value::E("room"), Fit::Clamped));
    }
}
