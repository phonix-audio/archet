//! Music theory utilities: scales, keys, pitch snapping, transposition.

/// Scale type for harmony and autotune.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScaleType {
    Major,
    Minor,
    Dorian,
    Mixolydian,
    Phrygian,
    Lydian,
    HarmonicMinor,
    MelodicMinor,
    PentatonicMajor,
    PentatonicMinor,
    Blues,
    Chromatic,
}

impl ScaleType {
    /// Semitone intervals from root for this scale.
    pub fn intervals(&self) -> &'static [u8] {
        match self {
            ScaleType::Major => &[0, 2, 4, 5, 7, 9, 11],
            ScaleType::Minor => &[0, 2, 3, 5, 7, 8, 10],
            ScaleType::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            ScaleType::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            ScaleType::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            ScaleType::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            ScaleType::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            ScaleType::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            ScaleType::PentatonicMajor => &[0, 2, 4, 7, 9],
            ScaleType::PentatonicMinor => &[0, 3, 5, 7, 10],
            ScaleType::Blues => &[0, 3, 5, 6, 7, 10],
            ScaleType::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        }
    }

    /// All scale types for iteration.
    pub fn all() -> &'static [ScaleType] {
        &[
            ScaleType::Major, ScaleType::Minor, ScaleType::Dorian,
            ScaleType::Mixolydian, ScaleType::Phrygian, ScaleType::Lydian,
            ScaleType::HarmonicMinor, ScaleType::MelodicMinor,
            ScaleType::PentatonicMajor, ScaleType::PentatonicMinor,
            ScaleType::Blues, ScaleType::Chromatic,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            ScaleType::Major => "Major",
            ScaleType::Minor => "Minor",
            ScaleType::Dorian => "Dorian",
            ScaleType::Mixolydian => "Mixolydian",
            ScaleType::Phrygian => "Phrygian",
            ScaleType::Lydian => "Lydian",
            ScaleType::HarmonicMinor => "Harmonic Minor",
            ScaleType::MelodicMinor => "Melodic Minor",
            ScaleType::PentatonicMajor => "Pentatonic Major",
            ScaleType::PentatonicMinor => "Pentatonic Minor",
            ScaleType::Blues => "Blues",
            ScaleType::Chromatic => "Chromatic",
        }
    }
}

/// Key note names.
pub const KEY_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Get the note name for a MIDI note number.
pub fn midi_note_name(midi_note: f32) -> String {
    let note = midi_note.round() as i32;
    let name = KEY_NAMES[(note.rem_euclid(12)) as usize];
    let octave = (note / 12) - 1;
    format!("{}{}", name, octave)
}

/// Snap a fractional MIDI note to the nearest note in the given key/scale.
/// Returns the snapped MIDI note (fractional part preserved as 0).
pub fn snap_to_scale(midi_note: f32, key: u8, scale: ScaleType) -> f32 {
    let intervals = scale.intervals();
    let note_in_octave = midi_note.rem_euclid(12.0);
    let octave = (midi_note / 12.0).floor() * 12.0;
    let relative = note_in_octave - key as f32;
    let relative_pos = relative.rem_euclid(12.0);

    // Find nearest scale degree
    let mut best_dist = f32::MAX;
    let mut best_note = midi_note.round();

    for &interval in intervals {
        let scale_note = interval as f32;
        // Check this octave and adjacent
        for offset in [-12.0f32, 0.0, 12.0] {
            let candidate = scale_note + offset;
            let dist = (relative_pos - candidate).abs();
            if dist < best_dist {
                best_dist = dist;
                best_note = octave + key as f32 + candidate;
                // Normalize to correct octave
                while best_note < midi_note - 6.0 {
                    best_note += 12.0;
                }
                while best_note > midi_note + 6.0 {
                    best_note -= 12.0;
                }
            }
        }
    }

    best_note
}

/// Transpose a MIDI note by a number of scale degrees within the given key/scale.
/// Positive degrees = up, negative = down.
pub fn transpose_in_scale(midi_note: f32, key: u8, scale: ScaleType, degrees: i8) -> f32 {
    let intervals = scale.intervals();
    let num_degrees = intervals.len() as i32;

    // Find the current scale degree
    let note_class = (midi_note.round() as i32).rem_euclid(12);
    let relative = (note_class - key as i32).rem_euclid(12) as u8;

    // Find nearest scale degree index
    let mut degree_idx = 0i32;
    let mut min_dist = 12u8;
    for (i, &interval) in intervals.iter().enumerate() {
        let dist = (relative as i8 - interval as i8).unsigned_abs();
        if dist < min_dist {
            min_dist = dist;
            degree_idx = i as i32;
        }
    }

    // Transpose by degrees
    let target_degree = degree_idx + degrees as i32;
    let octave_offset = target_degree.div_euclid(num_degrees);
    let target_idx = target_degree.rem_euclid(num_degrees) as usize;

    let base_octave = (midi_note / 12.0).floor() * 12.0;

    // Re-center the snapped current note within ±6 semitones of the input
    // (same normalization as snap_to_scale). Without this, any key whose
    // scale degrees land above the input's octave boundary (key != C)
    // resolves an octave too high.
    let mut current_note = base_octave + intervals[degree_idx as usize] as f32 + key as f32;
    while current_note < midi_note - 6.0 {
        current_note += 12.0;
    }
    while current_note > midi_note + 6.0 {
        current_note -= 12.0;
    }

    let semitone_delta = intervals[target_idx] as f32 - intervals[degree_idx as usize] as f32
        + octave_offset as f32 * 12.0;
    current_note + semitone_delta
}

/// Find the nearest note from a set of chord notes (MIDI note numbers).
pub fn nearest_chord_tone(midi_note: f32, chord_notes: &[u8]) -> f32 {
    if chord_notes.is_empty() {
        return midi_note.round();
    }

    let _note_class = midi_note.rem_euclid(12.0);
    let octave = (midi_note / 12.0).floor() * 12.0;

    let mut best_dist = f32::MAX;
    let mut best_note = midi_note.round();

    for &cn in chord_notes {
        let cn_class = (cn % 12) as f32;
        // Check nearest octave placement
        for oct_offset in [-12.0f32, 0.0, 12.0] {
            let candidate = octave + cn_class + oct_offset;
            let dist = (candidate - midi_note).abs();
            if dist < best_dist {
                best_dist = dist;
                best_note = candidate;
            }
        }
    }

    best_note
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snap_c_major() {
        // C4 = 60, should snap to itself
        assert_eq!(snap_to_scale(60.0, 0, ScaleType::Major), 60.0);
        // C#4 = 61, should snap to C or D (nearest in C major)
        let snapped = snap_to_scale(61.0, 0, ScaleType::Major);
        assert!(snapped == 60.0 || snapped == 62.0, "C# should snap to C or D, got {}", snapped);
    }

    #[test]
    fn test_transpose_third_up() {
        // C4 (60) up a 3rd in C major = E4 (64)
        let result = transpose_in_scale(60.0, 0, ScaleType::Major, 2);
        assert_eq!(result, 64.0, "C4 + 3rd in C major should be E4 (64), got {}", result);
    }

    #[test]
    fn test_transpose_diatonic() {
        // D4 (62) up a 3rd in C major = F4 (65), not F# (minor 3rd vs major 3rd)
        let result = transpose_in_scale(62.0, 0, ScaleType::Major, 2);
        assert_eq!(result, 65.0, "D4 + 3rd in C major should be F4 (65), got {}", result);
    }

    #[test]
    fn test_transpose_third_up_g_major() {
        // D4 (62) up a diatonic 3rd in G major = F#4 (66), NOT F#5 (78).
        // Regression: keys != C used to resolve an octave too high.
        let result = transpose_in_scale(62.0, 7, ScaleType::Major, 2);
        assert_eq!(result, 66.0, "D4 + 3rd in G major should be F#4 (66), got {}", result);
    }

    #[test]
    fn test_transpose_third_up_bb_major() {
        // F4 (65) up a diatonic 3rd in Bb major (key=10) = A4 (69), NOT A5 (81).
        let result = transpose_in_scale(65.0, 10, ScaleType::Major, 2);
        assert_eq!(result, 69.0, "F4 + 3rd in Bb major should be A4 (69), got {}", result);
    }

    #[test]
    fn test_transpose_third_down_g_major() {
        // G4 (67) down a diatonic 3rd in G major = E4 (64).
        let result = transpose_in_scale(67.0, 7, ScaleType::Major, -2);
        assert_eq!(result, 64.0, "G4 - 3rd in G major should be E4 (64), got {}", result);
    }

    #[test]
    fn test_transpose_stays_near_input_all_keys() {
        // Unison transposition (0 degrees) must never move more than a
        // snap distance (±6 st) away from the input, in ANY key.
        for key in 0u8..12 {
            for note in 48..72 {
                let result = transpose_in_scale(note as f32, key, ScaleType::Minor, 0);
                let dist = (result - note as f32).abs();
                assert!(dist <= 6.0,
                    "key={} note={} snapped {} st away (to {})", key, note, dist, result);
            }
        }
    }

    #[test]
    fn test_nearest_chord_tone() {
        // C major chord: C(0), E(4), G(7)
        let chord = vec![60, 64, 67]; // C4, E4, G4
        // D4 (62) should snap to C4 (60) or E4 (64)
        let result = nearest_chord_tone(62.0, &chord);
        assert!(result == 60.0 || result == 64.0, "D should snap to C or E, got {}", result);
    }

    #[test]
    fn test_midi_note_name() {
        assert_eq!(midi_note_name(60.0), "C4");
        assert_eq!(midi_note_name(69.0), "A4");
        assert_eq!(midi_note_name(61.0), "C#4");
    }
}
