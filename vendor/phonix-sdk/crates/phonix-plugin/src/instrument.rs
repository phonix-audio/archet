//! The contract an instrument offers a host that runs it in process.
//!
//! A plugin binary implements the same surface against its framework; a host
//! that links an instrument directly, as the sequencer does with its own,
//! reaches it through this trait rather than through a name of its own.

use crate::identity::Identity;

pub trait Instrument: Send {
    fn identity(&self) -> Identity;
    fn prepare(&mut self, sample_rate: f32, max_block: usize);
    fn note_on(&mut self, note: u8, velocity: u8);
    fn note_off(&mut self, note: u8);
    /// One block, planar: `out[channel][frame]`.
    fn process(&mut self, out: &mut [&mut [f32]]);
    /// The state a project stores, as the bytes the plugin's own persist key
    /// carries.
    fn save_state(&self) -> Vec<u8>;
    fn load_state(&mut self, state: &[u8]);
}
