//! Arpeggiator output event — generic, no plugin-specific dependencies.

/// A single event emitted by the arpeggiator.
#[derive(Debug, Clone, Copy)]
pub struct ArpEvent {
    /// MIDI note number (0-127)
    pub note: u8,
    /// Velocity (0-127)
    pub velocity: u8,
    /// Pan position (-1.0 to 1.0)
    pub pan: f32,
    /// Whether this is a note-on (true) or note-off (false)
    pub is_on: bool,
    /// Gate length override for this event (0.0 = use global)
    pub gate_length: f32,
    /// This note is a tie (extend previous, no retrigger)
    pub is_tie: bool,
    /// This note is accented
    pub is_accent: bool,
    /// Voice ID for tracking (chord/wide mode)
    pub voice_id: Option<u32>,
    /// Ratchet subdivision index (0-based)
    pub ratchet_index: u8,
    /// Total ratchet subdivisions (1 = normal, 2-4 = ratchet)
    pub ratchet_count: u8,
}

impl ArpEvent {
    /// Create a note-on event
    pub fn note_on(note: u8, velocity: u8, pan: f32) -> Self {
        Self {
            note,
            velocity,
            pan,
            is_on: true,
            gate_length: 0.0,
            is_tie: false,
            is_accent: false,
            voice_id: None,
            ratchet_index: 0,
            ratchet_count: 1,
        }
    }

    /// Create a note-off event
    pub fn note_off(note: u8) -> Self {
        Self {
            note,
            velocity: 0,
            pan: 0.0,
            is_on: false,
            gate_length: 0.0,
            is_tie: false,
            is_accent: false,
            voice_id: None,
            ratchet_index: 0,
            ratchet_count: 1,
        }
    }
}

/// Backward-compatible alias
pub type ArpNote = ArpEvent;
