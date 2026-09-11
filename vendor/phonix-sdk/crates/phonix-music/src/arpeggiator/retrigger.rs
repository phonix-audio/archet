//! Retrigger/reset modes — control when the arpeggiator pattern resets.

/// When the arpeggiator pattern resets to step 0
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetriggerMode {
    /// Free-running, never resets automatically
    Never,
    /// Reset to step 0 when a new note is pressed
    OnNewNote,
    /// Reset on every bar boundary (requires transport info)
    EveryBar,
    /// Reset every N beats (1–16)
    EveryNBeats(u8),
}

impl RetriggerMode {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Never,
            1 => Self::OnNewNote,
            2 => Self::EveryBar,
            n => Self::EveryNBeats((n - 2).clamp(1, 16) as u8),
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Self::Never => 0,
            Self::OnNewNote => 1,
            Self::EveryBar => 2,
            Self::EveryNBeats(n) => 2 + *n as usize,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Never => "Never",
            Self::OnNewNote => "On New Note",
            Self::EveryBar => "Every Bar",
            Self::EveryNBeats(1) => "Every Beat",
            Self::EveryNBeats(2) => "Every 2 Beats",
            Self::EveryNBeats(4) => "Every 4 Beats",
            Self::EveryNBeats(8) => "Every 8 Beats",
            Self::EveryNBeats(_) => "Every N Beats",
        }
    }
}
