//! Shared scale tables (pub so the piano roll and the generators reuse them).

pub const CHROMATIC:    &[u8] = &[0,1,2,3,4,5,6,7,8,9,10,11];
pub const MAJOR:        &[u8] = &[0,2,4,5,7,9,11];
pub const MINOR:        &[u8] = &[0,2,3,5,7,8,10];
pub const PENTATONIC:   &[u8] = &[0,2,4,7,9];
pub const BLUES:        &[u8] = &[0,3,5,6,7,10];
pub const DORIAN:       &[u8] = &[0,2,3,5,7,9,10];
pub const PHRYGIAN:     &[u8] = &[0,1,3,5,7,8,10];
pub const LYDIAN:       &[u8] = &[0,2,4,6,7,9,11];
pub const MIXOLYDIAN:   &[u8] = &[0,2,4,5,7,9,10];

pub const SCALE_NAMES: &[&str] = &[
    "Chromatic","Major","Minor","Pentatonic","Blues","Dorian","Phrygian","Lydian","Mixolydian",
];
pub const SCALES: &[&[u8]] = &[
    CHROMATIC, MAJOR, MINOR, PENTATONIC, BLUES,
    DORIAN, PHRYGIAN, LYDIAN, MIXOLYDIAN,
];

