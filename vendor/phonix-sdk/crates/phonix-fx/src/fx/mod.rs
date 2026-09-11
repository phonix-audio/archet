//! Convolution reverb, and the impulse responses it runs.
//!
//! Kept apart from `effects`, which is the registry of insert effects a rack
//! draws and a chain spec names. This is a reverb an instrument runs inside
//! itself, as an alternative to the algorithmic one -- `ReverbKind` is what
//! chooses between the two.

pub mod convolution;
pub mod irs;
