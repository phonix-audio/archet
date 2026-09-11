//! Groove template — the serializable micro-timing feel of a bar.
//!
//! Extraction (spectral-flux onset detection over an audio clip) needs
//! decoded audio, so it stays host-side; the template itself is data.

use serde::{Deserialize, Serialize};

/// 16th-note grid: 4 per beat, 16 per 4/4 bar.
pub const SUBDIVS: usize = 16;

/// Per-subdivision micro-timing template. `dev_ticks[i]` = how far the feel
/// pushes a note landing on subdivision `i` of the bar, in PPQN ticks
/// (positive = late / behind the beat). Zero-filled subdivisions mean "no
/// onset observed there": those notes are left on the grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrooveTemplate {
    pub dev_ticks: Vec<i32>,
    /// Onsets observed per subdivision (confidence: 0 = leave on grid).
    pub weight:    Vec<u16>,
}

impl GrooveTemplate {
    pub fn is_empty(&self) -> bool {
        self.weight.iter().all(|&w| w == 0)
    }
}
