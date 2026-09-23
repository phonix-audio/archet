//! The unit's display, written region by region: a name of eight
//! characters for the track and one for the device, a parameter line of
//! sixteen, eight encoder segments of five characters on two rows, and a
//! label of five under each of the four keys beneath the display.
//!
//! Every message is `SetDisplay` with a region, a fourteen-bit offset into
//! it and the text; two runs can share one message. The layout is the one
//! the legacy driver wrote the display with.

use super::{sysex, Command};

/// How many encoders the display has a segment for.
pub const ENCODERS: usize = super::ENCODERS as usize;
/// Characters in one encoder segment, label or value.
pub const SEGMENT: usize = 5;
/// Characters in one row of encoder segments.
pub const ROW: usize = ENCODERS * SEGMENT;
/// Characters in the track and device names.
pub const NAME: usize = 8;
/// Characters in the parameter line.
pub const PARAMETER: usize = 16;
/// Keys under the display, each with a label of `SEGMENT` characters.
pub const PAGE_KEYS: usize = 4;
/// Where the device name starts in the name region.
pub const DEVICE_NAME_OFFSET: usize = 0x0A;

const PRINTABLE: std::ops::RangeInclusive<u8> = 0x20..=0x7E;
const SEVEN_BITS: usize = 0x7F;

/// The regions a message addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Region {
    TrackDevice = 0x01,
    Parameter = 0x02,
    Encoders = 0x03,
    PageKeys = 0x04,
}

/// `text` as the display can show it: printable ASCII only, anything else
/// a space, cut to `width` and centred in it. Always exactly `width` bytes.
pub fn fit(text: &str, width: usize) -> String {
    let clean: String = text
        .chars()
        .map(|c| if c.is_ascii() && PRINTABLE.contains(&(c as u8)) { c } else { ' ' })
        .take(width)
        .collect();
    let clean = clean.trim();
    format!("{clean:^width$}")
}

/// Blank every region.
pub fn clear() -> Vec<u8> {
    sysex(Command::ClearDisplay, &[])
}

/// One run of text at `offset` in `region`, as payload bytes.
fn run(payload: &mut Vec<u8>, offset: usize, text: &str) {
    payload.push(((offset >> 7) & SEVEN_BITS) as u8);
    payload.push((offset & SEVEN_BITS) as u8);
    payload.extend_from_slice(text.as_bytes());
}

fn set(region: Region, offset: usize, text: &str) -> Vec<u8> {
    let mut payload = vec![region as u8];
    run(&mut payload, offset, text);
    sysex(Command::SetDisplay, &payload)
}

/// The track name, `NAME` characters.
pub fn track_name(name: &str) -> Vec<u8> {
    set(Region::TrackDevice, 0, &fit(name, NAME))
}

/// The device name, `NAME` characters beside the track's.
pub fn device_name(name: &str) -> Vec<u8> {
    set(Region::TrackDevice, DEVICE_NAME_OFFSET, &fit(name, NAME))
}

/// The parameter line, `PARAMETER` characters.
pub fn parameter(text: &str) -> Vec<u8> {
    set(Region::Parameter, 0, &fit(text, PARAMETER))
}

/// The label under key `index`, 0..3.
pub fn page_key(index: u8, label: &str) -> Vec<u8> {
    let index = (index as usize).min(PAGE_KEYS - 1);
    let mut payload = vec![Region::PageKeys as u8, index as u8, 0];
    payload.extend_from_slice(fit(label, SEGMENT).as_bytes());
    sysex(Command::SetDisplay, &payload)
}

/// Encoder `index`'s segment, 0..7: `label` on the top row, `value` under it.
pub fn encoder_segment(index: u8, label: &str, value: &str) -> Vec<u8> {
    let offset = (index as usize).min(ENCODERS - 1) * SEGMENT;
    let mut payload = vec![Region::Encoders as u8];
    run(&mut payload, offset, &fit(label, SEGMENT));
    run(&mut payload, offset + ROW, &fit(value, SEGMENT));
    sysex(Command::SetDisplay, &payload)
}

/// Both encoder rows at once, `ROW` characters each.
pub fn encoder_rows(top: &str, bottom: &str) -> Vec<u8> {
    let mut payload = vec![Region::Encoders as u8];
    run(&mut payload, 0, &fit(top, ROW));
    run(&mut payload, ROW, &fit(bottom, ROW));
    sysex(Command::SetDisplay, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypercontrol::HEADER;

    /// Text is cut or centred to the width, and never leaves ASCII.
    #[test]
    fn text_fits_its_width() {
        assert_eq!(fit("Cutoff", 5), "Cutof");
        assert_eq!(fit("Hz", 5), " Hz  ");
        assert_eq!(fit("", 5), "     ");
        assert_eq!(fit("caf\u{e9}", 5), " caf ");
        assert_eq!(fit("a\tb", 5), " a b ");
        for w in 0..20 {
            let s = fit("Some longer text", w);
            assert_eq!(s.len(), w);
            assert!(s.bytes().all(|b| b < 0x80));
        }
    }

    /// A segment writes its label at the encoder's column and its value one
    /// row below, five bytes each.
    #[test]
    fn a_segment_addresses_its_column_on_both_rows() {
        let m = encoder_segment(3, "Cutoff", "64");
        assert_eq!(&m[..6], &HEADER);
        assert_eq!(m[6], Command::SetDisplay as u8);
        assert_eq!(m[7], Region::Encoders as u8);
        assert_eq!(&m[8..10], &[0x00, 15]);
        assert_eq!(&m[10..15], b"Cutof");
        assert_eq!(&m[15..17], &[0x00, 55]);
        assert_eq!(&m[17..22], b" 64  ");
        assert_eq!(m[22], 0xF7);
        assert_eq!(m.len(), 23);
        assert_eq!(&encoder_segment(9, "", "")[8..10], &[0x00, 35], "clamped to the last encoder");
    }

    /// The other regions carry their width and their offset.
    #[test]
    fn regions_carry_their_width() {
        assert_eq!(clear(), [0xF0, 0x00, 0x01, 0x05, 0x20, 0x7F, 0x10, 0xF7]);
        let p = parameter("Cutoff");
        assert_eq!(p[7], Region::Parameter as u8);
        assert_eq!(p.len(), HEADER.len() + 1 + 1 + 2 + PARAMETER + 1);
        let d = device_name("Scena");
        assert_eq!(&d[7..10], &[Region::TrackDevice as u8, 0x00, DEVICE_NAME_OFFSET as u8]);
        assert_eq!(&d[10..18], b" Scena  ");
        let t = track_name("Piano");
        assert_eq!(&t[7..10], &[Region::TrackDevice as u8, 0x00, 0x00]);
        let k = page_key(2, "2/5");
        assert_eq!(&k[7..10], &[Region::PageKeys as u8, 2, 0]);
        assert_eq!(&k[10..15], b" 2/5 ");
        let r = encoder_rows("top", "bottom");
        assert_eq!(r.len(), HEADER.len() + 1 + 1 + (2 + ROW) * 2 + 1);
        assert_eq!(&r[8 + 2 + ROW..8 + 2 + ROW + 2], &[0x00, ROW as u8]);
    }
}
