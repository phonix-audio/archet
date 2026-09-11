//! The effects layer: what every effect implements (`effect`), how it
//! describes itself (`spec`), the kinds a build knows (`registry`), slots in
//! series (`chain`) and the description a preset, a patch or a session
//! stores (`chain_spec`). Every effect sits behind a cargo feature; with
//! none on, this is the contract alone.

pub mod chain;
pub mod chain_spec;
pub mod effect;
pub mod effects;
pub mod preset;
pub mod registry;
pub mod report;
pub mod reverb_kind;
pub mod spec;
pub mod trance_gate;

pub use chain::{Chain, ParamRef};
pub use chain_spec::{ChainSpec, MacroSpec, MacroTarget, SlotSpec, SpecValue};
pub use effect::{Effect, Musical, Ports, Stereo, StereoMut, Transport, Value};
pub use preset::ChainPreset;
pub use registry::{Entry, Registry};
pub use report::ApplyReport;
pub use reverb_kind::ReverbKind;
pub use spec::{Category, Curve, EffectSpec, Needs, ParamFlags, ParamKind, ParamSpec, ReadoutKind, ReadoutSpec, Unit};
