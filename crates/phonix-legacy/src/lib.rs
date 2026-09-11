//! The effect state written before kinds and parameters had names: an
//! ordinal per slot and `(pid, value)` pairs in a flat aliased space. This
//! crate redefines those shapes, holds the tables that name them, and
//! rewrites them as `ChainSpec`. Nothing else reads the old space.

use phonix_fx::{ChainSpec, MacroSpec, MacroTarget, SlotSpec, SpecValue};
use serde::{Deserialize, Serialize};

pub mod kinds;
pub mod params;

pub use kinds::{kind_of, KINDS};
pub use params::{Rule, PARAMS};

/// One insert slot as every session and patch wrote it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct FxSlotState {
    /// The ordinal in the historical effect list; 0 is an empty slot.
    pub effect_type: u8,
    pub enabled: bool,
    pub mix: f32,
    #[serde(default)]
    pub params: Vec<(u8, f32)>,
    /// A key source track index; `None` is self-keyed.
    #[serde(default)]
    pub sidechain_src: Option<u16>,
    /// The first chain of a rack.
    #[serde(default)]
    pub nested: Vec<FxSlotState>,
    #[serde(default)]
    pub rack: RackState,
}

/// A macro's hold on one parameter of a rack's chains.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct MacroMap {
    pub macro_idx: u8,
    pub chain: u8,
    pub slot: u8,
    pub pid: u8,
    pub lo: f32,
    pub hi: f32,
}

/// The chains of a rack beside the first, their gains and selector zones,
/// the selector, eight macros and what they hold.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RackState {
    #[serde(default)]
    pub chains: Vec<Vec<FxSlotState>>,
    #[serde(default = "RackState::default_gains")]
    pub gains: Vec<f32>,
    #[serde(default = "RackState::default_zones")]
    pub zones: Vec<(u8, u8)>,
    #[serde(default)]
    pub selector: u8,
    #[serde(default)]
    pub macros: [f32; 8],
    #[serde(default)]
    pub maps: Vec<MacroMap>,
}

impl RackState {
    fn default_gains() -> Vec<f32> {
        vec![1.0]
    }
    fn default_zones() -> Vec<(u8, u8)> {
        vec![(0, 127)]
    }
}

impl Default for RackState {
    fn default() -> Self {
        RackState {
            chains: Vec::new(),
            gains: Self::default_gains(),
            zones: Self::default_zones(),
            selector: 0,
            macros: [0.0; 8],
            maps: Vec::new(),
        }
    }
}

/// The kind of a rack slot in the new space.
pub const RACK: &str = "rack";

/// How a sidechain track index is written in a `SlotSpec`.
pub fn sidechain_name(track: u16) -> String {
    format!("track:{track}")
}

/// A `(kind, pid)` pair the tables do not name; its value is dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unconverted {
    pub kind: &'static str,
    pub pid: u8,
}

/// What a conversion could not carry across.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Ordinals outside the historical list; the slot is dropped.
    pub unknown_kinds: Vec<u8>,
    pub unconverted: Vec<Unconverted>,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.unknown_kinds.is_empty() && self.unconverted.is_empty()
    }
    fn absorb(&mut self, other: Report) {
        self.unknown_kinds.extend(other.unknown_kinds);
        self.unconverted.extend(other.unconverted);
    }
}

/// Rewrites a list of old slots as a chain. Empty slots (ordinal 0) are
/// dropped, so slot numbers in the result may differ from the old ones.
pub fn convert_chain(slots: &[FxSlotState]) -> (ChainSpec, Report) {
    let mut report = Report::default();
    let mut out = Vec::with_capacity(slots.len());
    for slot in slots {
        if slot.effect_type == 0 {
            continue;
        }
        let Some(kind) = kind_of(slot.effect_type) else {
            report.unknown_kinds.push(slot.effect_type);
            continue;
        };
        let (spec, r) = convert_slot(kind, slot);
        report.absorb(r);
        out.push(spec);
    }
    (ChainSpec::new(out), report)
}

fn convert_slot(kind: &'static str, slot: &FxSlotState) -> (SlotSpec, Report) {
    let mut report = Report::default();
    let mut spec = SlotSpec::new(kind).enabled(slot.enabled).mix(slot.mix);
    spec.sidechain = slot.sidechain_src.map(sidechain_name);
    for &(pid, value) in &slot.params {
        match params::lookup(kind, pid) {
            Some((id, Rule::BitsToBools(n))) => {
                let bits = value.round().max(0.0) as u32;
                for bit in 0..n as u32 {
                    spec.set(id.replace("{}", &bit.to_string()), bits & (1 << bit) != 0);
                }
            }
            Some((_, Rule::IntoSlotMix)) => spec.mix *= value.clamp(0.0, 1.0),
            Some((id, rule)) => {
                if let Some(v) = rule.apply(value) {
                    spec.set(id, v);
                }
            }
            None => report.unconverted.push(Unconverted { kind, pid }),
        }
    }
    if kind == RACK {
        let (first, r) = convert_chain(&slot.nested);
        report.absorb(r);
        spec.chains.push(first);
        for chain in &slot.rack.chains {
            let (c, r) = convert_chain(chain);
            report.absorb(r);
            spec.chains.push(c);
        }
        for (i, gain) in slot.rack.gains.iter().enumerate() {
            spec.set(format!("gain.{i}"), *gain);
        }
        for (i, (lo, hi)) in slot.rack.zones.iter().enumerate() {
            spec.set(format!("zone.{i}.low"), *lo as i64);
            spec.set(format!("zone.{i}.high"), *hi as i64);
        }
        spec.set("selector", slot.rack.selector as i64);
        spec.macros = (0..slot.rack.macros.len())
            .map(|m| MacroSpec {
                name: format!("Macro {}", m + 1),
                value: slot.rack.macros[m],
                targets: slot
                    .rack
                    .maps
                    .iter()
                    .filter(|map| map.macro_idx as usize == m)
                    .filter_map(|map| {
                        let chain = spec.chains.get(map.chain as usize)?;
                        let target = chain.slots.get(map.slot as usize)?;
                        let (id, _) = params::lookup(kind_by_name(&target.kind)?, map.pid)?;
                        Some(MacroTarget { chain: map.chain as usize, slot: map.slot as usize, id: id.to_string(), min: map.lo, max: map.hi })
                    })
                    .collect(),
            })
            .collect();
    }
    (spec, report)
}

fn kind_by_name(name: &str) -> Option<&'static str> {
    KINDS.iter().copied().find(|k| *k == name)
}

/// `SpecValue` for a rule's output.
impl Rule {
    /// The written value for `raw`; `None` when the value is the slot's mix
    /// and belongs on the slot, not in its parameters.
    pub fn apply(self, raw: f32) -> Option<SpecValue> {
        Some(match self {
            Rule::Direct => SpecValue::F(raw),
            Rule::Scale(k) => SpecValue::F(raw * k),
            Rule::Bool => SpecValue::B(raw > 0.5),
            Rule::Int => SpecValue::I(raw.round() as i64),
            Rule::Enum(variants) => {
                let i = (raw.round().max(0.0) as usize).min(variants.len().saturating_sub(1));
                SpecValue::S(variants.get(i).copied().unwrap_or("").to_string())
            }
            Rule::IntoSlotMix | Rule::BitsToBools(_) => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_slot_list_becomes_a_named_chain_without_the_empties() {
        let slots = vec![
            FxSlotState { effect_type: 20, enabled: true, mix: 1.0, params: vec![], sidechain_src: None, ..Default::default() },
            FxSlotState::default(),
            FxSlotState { effect_type: 8, enabled: false, mix: 0.5, params: vec![], sidechain_src: Some(3), ..Default::default() },
        ];
        let (chain, report) = convert_chain(&slots);
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(chain.kinds().collect::<Vec<_>>(), vec!["parametric-eq", "compressor"]);
        assert!(!chain.slots[1].enabled);
        assert_eq!(chain.slots[1].mix, 0.5);
        assert_eq!(chain.slots[1].sidechain.as_deref(), Some("track:3"));
    }

    #[test]
    fn the_cordis_chain_converts_with_nothing_left_behind() {
        let slots = vec![
            FxSlotState { effect_type: 20, enabled: true, mix: 1.0, params: vec![(28, 90.0), (38, 0.7), (29, -3.0), (80, 1.0), (79, 1.0)], ..Default::default() },
            FxSlotState { effect_type: 8, enabled: true, mix: 1.0, params: vec![(60, -18.0), (61, 2.0), (62, 0.020), (63, 0.150), (65, 6.0), (66, 1.0)], ..Default::default() },
            FxSlotState { effect_type: 2, enabled: true, mix: 1.0, params: vec![(16, 1.0), (10, 0.35), (11, 0.40), (12, 0.5), (13, 0.008), (15, 1.0), (14, 0.22)], ..Default::default() },
            FxSlotState { effect_type: 28, enabled: true, mix: 1.0, params: vec![(28, -0.3), (29, 50.0), (38, 1.0)], ..Default::default() },
        ];
        let (chain, report) = convert_chain(&slots);
        assert!(report.is_clean(), "{report:?}");
        let eq = &chain.slots[0];
        assert_eq!(eq.f32("band.0.freq"), Some(90.0));
        assert_eq!(eq.get("band.0.enabled"), Some(&SpecValue::B(true)));
        assert_eq!(eq.get("band.1.enabled"), Some(&SpecValue::B(false)));
        assert_eq!(eq.mix, 1.0);
        assert_eq!(chain.slots[1].variant("mode"), None, "the old chain never wrote a mode");
        let room = &chain.slots[2];
        assert_eq!(room.variant("type"), Some("room"));
        assert_eq!(room.mix, 0.22);
        assert_eq!(chain.slots[3].f32("ceiling"), Some(-0.3));
        assert_eq!(chain.slots[3].f32("release"), Some(50.0));
    }

    #[test]
    fn a_rack_carries_its_chains_gains_zones_and_macros() {
        let inner = FxSlotState { effect_type: 20, enabled: true, mix: 1.0, ..Default::default() };
        let mut rack = FxSlotState { effect_type: 45, enabled: true, mix: 1.0, nested: vec![inner.clone()], ..Default::default() };
        rack.rack.chains.push(vec![inner]);
        rack.rack.gains = vec![1.0, 0.5];
        rack.rack.zones = vec![(0, 63), (64, 127)];
        rack.rack.macros[0] = 0.25;
        let (chain, report) = convert_chain(&[rack]);
        assert!(report.is_clean(), "{report:?}");
        let slot = &chain.slots[0];
        assert_eq!(slot.kind, RACK);
        assert_eq!(slot.chains.len(), 2);
        assert_eq!(slot.f32("gain.1"), Some(0.5));
        assert_eq!(slot.get("zone.1.low"), Some(&SpecValue::I(64)));
        assert_eq!(slot.macros.len(), 8);
        assert_eq!(slot.macros[0].value, 0.25);
    }

    #[test]
    fn an_ordinal_past_the_list_and_an_unnamed_pid_are_reported() {
        let slots = vec![
            FxSlotState { effect_type: 200, enabled: true, mix: 1.0, ..Default::default() },
            FxSlotState { effect_type: 20, enabled: true, mix: 1.0, params: vec![(255, 0.5)], ..Default::default() },
        ];
        let (chain, report) = convert_chain(&slots);
        assert_eq!(report.unknown_kinds, vec![200]);
        assert_eq!(report.unconverted, vec![Unconverted { kind: "parametric-eq", pid: 255 }]);
        assert_eq!(chain.len(), 1);
    }
}
