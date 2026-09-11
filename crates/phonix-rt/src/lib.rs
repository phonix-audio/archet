//! What the audio thread is allowed to touch: no locks, no allocation, and
//! no dependencies. The state hand-off to a GUI, the denormal guard, and
//! delay compensation.

pub mod denormal;
pub mod handoff;
pub mod pdc;
pub mod triple_buffer;

pub use triple_buffer::*;
pub use handoff::Handoff;
