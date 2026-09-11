//! Shared FX data types.
//!
//! Serializable selectors an engine patch carries. No DSP: the
//! implementations live in the host `fx` module.

use serde::{Deserialize, Serialize};

/// Reverb topology selector. Plugins that offer both an algorithmic
/// (Schroeder) reverb and a convolution alternative dispatch on this
/// enum at process-time. Default is `Algorithmic` so existing patches
/// without the field deserialize to v3 behavior.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReverbKind {
    #[default]
    Algorithmic,
    Convolution,
}

impl ReverbKind {
    pub fn label(self) -> &'static str {
        match self {
            ReverbKind::Algorithmic => "Algorithmic",
            ReverbKind::Convolution => "Convolution",
        }
    }
    pub fn all() -> &'static [ReverbKind] {
        &[Self::Algorithmic, Self::Convolution]
    }
}
