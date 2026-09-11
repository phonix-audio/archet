//! A chain with a name: what a factory bank and a user's preset folder hold.

use serde::{Deserialize, Serialize};

use crate::chain_spec::ChainSpec;
use crate::registry::Registry;
use crate::report::{ApplyReport, Missing, Unknown, Clamped};
use crate::chain_spec::Fit;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainPreset {
    pub name: String,
    #[serde(default)]
    pub category: String,
    pub chain: ChainSpec,
}

impl ChainSpec {
    /// What `Chain::apply` would report, without building an effect.
    pub fn check(&self, registry: &Registry) -> ApplyReport {
        let mut report = ApplyReport::default();
        for (i, slot) in self.slots.iter().enumerate() {
            let Some(entry) = registry.get(&slot.kind) else {
                report.missing.push(Missing { slot: i, kind: slot.kind.clone() });
                continue;
            };
            for (id, value) in &slot.params {
                match entry.spec.param(id) {
                    Some(p) => {
                        if value.resolve(p).1 == Fit::Clamped {
                            report.clamped.push(Clamped { slot: i, id: id.clone() });
                        }
                    }
                    None => report.unknown.push(Unknown { slot: i, id: id.clone() }),
                }
            }
        }
        report
    }
}

impl ChainPreset {
    pub fn new(name: impl Into<String>, category: impl Into<String>, chain: ChainSpec) -> Self {
        ChainPreset { name: name.into(), category: category.into(), chain }
    }

    pub fn check(&self, registry: &Registry) -> ApplyReport {
        self.chain.check(registry)
    }
}
