//! Note values a tempo-synced parameter counts in.

/// A note value as a fraction of a whole note, plain, dotted or triplet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempoDivision {
    Whole,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    DottedHalf,
    DottedQuarter,
    DottedEighth,
    DottedSixteenth,
    TripletHalf,
    TripletQuarter,
    TripletEighth,
    TripletSixteenth,
}

/// The ids, as a parameter writes them.
pub const DIVISIONS: [&str; 14] = ["1-1", "1-2", "1-4", "1-8", "1-16", "1-32", "1-2d", "1-4d", "1-8d", "1-16d", "1-2t", "1-4t", "1-8t", "1-16t"];
pub const DIVISION_LABELS: [&str; 14] = ["1/1", "1/2", "1/4", "1/8", "1/16", "1/32", "1/2D", "1/4D", "1/8D", "1/16D", "1/2T", "1/4T", "1/8T", "1/16T"];

impl TempoDivision {
    pub const ALL: [TempoDivision; 14] = [
        Self::Whole, Self::Half, Self::Quarter, Self::Eighth, Self::Sixteenth, Self::ThirtySecond,
        Self::DottedHalf, Self::DottedQuarter, Self::DottedEighth, Self::DottedSixteenth,
        Self::TripletHalf, Self::TripletQuarter, Self::TripletEighth, Self::TripletSixteenth,
    ];

    pub fn named(name: &str) -> Self {
        DIVISIONS.iter().position(|d| *d == name).map(|i| Self::ALL[i]).unwrap_or(Self::Quarter)
    }

    pub fn name(self) -> &'static str {
        DIVISIONS[Self::ALL.iter().position(|d| *d == self).unwrap_or(2)]
    }

    /// Length in beats (quarter notes).
    pub fn beats(self) -> f32 {
        match self {
            Self::Whole => 4.0,
            Self::Half => 2.0,
            Self::Quarter => 1.0,
            Self::Eighth => 0.5,
            Self::Sixteenth => 0.25,
            Self::ThirtySecond => 0.125,
            Self::DottedHalf => 3.0,
            Self::DottedQuarter => 1.5,
            Self::DottedEighth => 0.75,
            Self::DottedSixteenth => 0.375,
            Self::TripletHalf => 4.0 / 3.0,
            Self::TripletQuarter => 2.0 / 3.0,
            Self::TripletEighth => 1.0 / 3.0,
            Self::TripletSixteenth => 0.5 / 3.0,
        }
    }

    pub fn to_time_seconds(self, tempo_bpm: f32) -> f32 {
        self.beats() * 60.0 / tempo_bpm.max(1.0)
    }
}
