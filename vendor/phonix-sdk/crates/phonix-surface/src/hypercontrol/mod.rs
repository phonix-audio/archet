//! HyperControl, the M-Audio Axiom Pro's second MIDI cable: control changes
//! on one channel for the encoders and the keys, system exclusives for the
//! handshake and the display. The controller numbers, the header and the
//! command bytes are those the AxiomPro remote script shipped with Ableton
//! Live uses.
//!
//! The unit speaks HyperControl only after it has answered the inquiry, and
//! leaves that mode on the disconnect message; a host sends the first when
//! it opens the port and the second when it closes it.

pub mod display;

use crate::{Event, Key};

/// The MIDI channel the controls arrive on and the lamps listen on, zero-based.
pub const CHANNEL: u8 = 14;
/// M-Audio's system exclusive manufacturer id.
pub const MANUFACTURER: [u8; 3] = [0x00, 0x01, 0x05];
/// The HyperControl product family byte.
pub const DEVICE: u8 = 0x20;
/// The unit address every unit answers to.
pub const BROADCAST: u8 = 0x7F;
/// Every HyperControl system exclusive opens with these six bytes.
pub const HEADER: [u8; 6] = [SYSEX_START, MANUFACTURER[0], MANUFACTURER[1], MANUFACTURER[2], DEVICE, BROADCAST];
/// How many encoders the surface has.
pub const ENCODERS: u8 = 8;
/// The handshake reply of a unit with sliders, the Axiom Pro 49 and 61.
pub const HAS_SLIDERS: u8 = 0x2E;
/// The handshake reply of a unit without, the Axiom Pro 25.
pub const NO_SLIDERS: u8 = 0x26;
/// What the legacy Windows driver names the virtual port that carries this cable.
pub const PORT_MARKER: &str = "HyperControl";
/// What ALSA appends to the unit's name for its second cable.
pub const ALSA_CABLE: &str = "MIDI 2";
/// The unit's name as it appears in a port name.
pub const UNIT: &str = "Axiom Pro";

const STATUS_CC: u8 = 0xB0;
const SYSEX_START: u8 = 0xF0;
const SYSEX_END: u8 = 0xF7;
/// The lowest value that means a turn anticlockwise: the encoders send a
/// two's complement delta in seven bits.
const ENCODER_NEGATIVE: u8 = 64;
const SEVEN_BITS: u8 = 0x7F;
const LAMP_ON: u8 = 0x7F;
const LAMP_OFF: u8 = 0x00;
const ON: u8 = 0x7F;
const OFF: u8 = 0x00;

/// The controller numbers the surface uses, all on `CHANNEL`.
pub mod cc {
    pub const MUTE: u8 = 12;
    pub const SOLO: u8 = 13;
    pub const BANK_RIGHT: u8 = 14;
    pub const BANK_LEFT: u8 = 15;
    /// Encoders 1 to 8 are 17 to 24.
    pub const ENCODER_1: u8 = 17;
    /// Select keys 1 to 8 are 49 to 56.
    pub const SELECT_1: u8 = 49;
    pub const MODE: u8 = 57;
    /// The four keys under the display are 74 to 77.
    pub const DEVICE_1: u8 = 74;
    pub const PEEK: u8 = 78;
    pub const DISPLAY: u8 = 79;
    pub const MIXER: u8 = 109;
    pub const SELECT_RIGHT: u8 = 110;
    pub const SELECT_LEFT: u8 = 111;
    pub const LOOP: u8 = 113;
    pub const REWIND: u8 = 114;
    pub const FAST_FORWARD: u8 = 115;
    pub const STOP: u8 = 116;
    pub const PLAY: u8 = 117;
    pub const RECORD: u8 = 118;
}

/// The system exclusive commands a host sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    ClearDisplay = 0x10,
    SetDisplay = 0x11,
    Handshake = 0x20,
}

/// What the unit sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Incoming {
    Control(Event),
    /// The unit answered the inquiry and is in HyperControl mode.
    Handshake { has_sliders: bool },
}

/// The message one complete MIDI message carries, if the unit sent one.
/// A control change on another channel, a note, or a system exclusive of
/// another maker is never a message.
pub fn parse(msg: &[u8]) -> Option<Incoming> {
    match msg {
        [status, number, value] if *status == STATUS_CC | CHANNEL => {
            parse_cc(*number, *value).map(Incoming::Control)
        }
        [SYSEX_START, .., SYSEX_END] => parse_sysex(msg),
        _ => None,
    }
}

fn parse_cc(number: u8, value: u8) -> Option<Event> {
    if let Some(index) = encoder_of(number) {
        let delta = if value < ENCODER_NEGATIVE { value as i16 } else { value as i16 - 128 } as i8;
        return (delta != 0).then_some(Event::Encoder { index, delta });
    }
    key_of(number).map(|key| Event::Key { key, down: value > 0 })
}

fn parse_sysex(msg: &[u8]) -> Option<Incoming> {
    let body = msg.strip_prefix(&HEADER[..])?.strip_suffix(&[SYSEX_END])?;
    match body {
        [command, reply, ..] if *command == Command::Handshake as u8 => match *reply {
            HAS_SLIDERS => Some(Incoming::Handshake { has_sliders: true }),
            NO_SLIDERS => Some(Incoming::Handshake { has_sliders: false }),
            _ => None,
        },
        _ => None,
    }
}

/// The encoder a controller number belongs to, 0..7.
pub fn encoder_of(number: u8) -> Option<u8> {
    number.checked_sub(cc::ENCODER_1).filter(|i| *i < ENCODERS)
}

/// The key a controller number is.
pub fn key_of(number: u8) -> Option<Key> {
    Some(match number {
        cc::PLAY => Key::Play,
        cc::STOP => Key::Stop,
        cc::RECORD => Key::Record,
        cc::LOOP => Key::Loop,
        cc::REWIND => Key::Rewind,
        cc::FAST_FORWARD => Key::FastForward,
        cc::BANK_LEFT => Key::BankLeft,
        cc::BANK_RIGHT => Key::BankRight,
        cc::PEEK => Key::Peek,
        cc::DISPLAY => Key::Display,
        cc::MUTE => Key::Mute,
        cc::SOLO => Key::Solo,
        cc::MIXER => Key::Mixer,
        cc::SELECT_LEFT => Key::SelectLeft,
        cc::SELECT_RIGHT => Key::SelectRight,
        cc::MODE => Key::Mode,
        n if (cc::SELECT_1..cc::SELECT_1 + ENCODERS).contains(&n) => Key::Select(n - cc::SELECT_1),
        n if (cc::DEVICE_1..cc::DEVICE_1 + display::PAGE_KEYS as u8).contains(&n) => Key::Device(n - cc::DEVICE_1),
        _ => return None,
    })
}

/// The controller number a key sends.
pub fn key_cc(key: Key) -> u8 {
    match key {
        Key::Play => cc::PLAY,
        Key::Stop => cc::STOP,
        Key::Record => cc::RECORD,
        Key::Loop => cc::LOOP,
        Key::Rewind => cc::REWIND,
        Key::FastForward => cc::FAST_FORWARD,
        Key::BankLeft => cc::BANK_LEFT,
        Key::BankRight => cc::BANK_RIGHT,
        Key::Select(i) => cc::SELECT_1 + i.min(ENCODERS - 1),
        Key::Device(i) => cc::DEVICE_1 + i.min(display::PAGE_KEYS as u8 - 1),
        Key::Peek => cc::PEEK,
        Key::Display => cc::DISPLAY,
        Key::Mute => cc::MUTE,
        Key::Solo => cc::SOLO,
        Key::Mixer => cc::MIXER,
        Key::SelectLeft => cc::SELECT_LEFT,
        Key::SelectRight => cc::SELECT_RIGHT,
        Key::Mode => cc::MODE,
    }
}

/// A system exclusive to the unit: the header, the command, the payload
/// cut to seven bits, the end byte.
pub fn sysex(command: Command, payload: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(HEADER.len() + 1 + payload.len() + 1);
    msg.extend_from_slice(&HEADER);
    msg.push(command as u8);
    msg.extend(payload.iter().map(|b| b & SEVEN_BITS));
    msg.push(SYSEX_END);
    msg
}

/// The inquiry that puts the unit in HyperControl mode. It answers with a
/// `Handshake`.
pub fn inquiry() -> Vec<u8> {
    sysex(Command::Handshake, &[HAS_SLIDERS])
}

/// The message that takes the unit out of HyperControl mode.
pub fn disconnect() -> Vec<u8> {
    sysex(Command::Handshake, &[OFF])
}

/// A control change to the unit on its channel.
pub fn cc(number: u8, value: u8) -> [u8; 3] {
    [STATUS_CC | CHANNEL, number & SEVEN_BITS, value & SEVEN_BITS]
}

/// The message lighting or darkening a key's lamp. Only the six transport
/// keys have one.
pub fn lamp(key: Key, on: bool) -> Option<[u8; 3]> {
    let lit = matches!(key, Key::Play | Key::Stop | Key::Record | Key::Loop | Key::Rewind | Key::FastForward);
    lit.then(|| cc(key_cc(key), if on { LAMP_ON } else { LAMP_OFF }))
}

/// The message turning the display on or off.
pub fn display_power(on: bool) -> [u8; 3] {
    cc(cc::DISPLAY, if on { ON } else { OFF })
}

/// Whether a MIDI port carries this cable: the legacy driver's virtual port
/// on Windows, or the unit's second port as ALSA names it.
pub fn is_port(name: &str) -> bool {
    name.contains(PORT_MARKER) || (name.contains(UNIT) && name.contains(ALSA_CABLE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(number: u8, value: u8) -> Option<Event> {
        match parse(&[STATUS_CC | CHANNEL, number, value]) {
            Some(Incoming::Control(e)) => Some(e),
            _ => None,
        }
    }

    /// An encoder sends a seven-bit two's complement delta; zero is not a turn.
    #[test]
    fn encoders_turn_by_signed_detents() {
        assert_eq!(control(17, 1), Some(Event::Encoder { index: 0, delta: 1 }));
        assert_eq!(control(17, 3), Some(Event::Encoder { index: 0, delta: 3 }));
        assert_eq!(control(17, 127), Some(Event::Encoder { index: 0, delta: -1 }));
        assert_eq!(control(17, 126), Some(Event::Encoder { index: 0, delta: -2 }));
        assert_eq!(control(24, 64), Some(Event::Encoder { index: 7, delta: -64 }));
        assert_eq!(control(17, 0), None);
        assert_eq!(control(25, 1), None, "past the last encoder");
    }

    /// A key sends its controller at full value down and zero up.
    #[test]
    fn keys_go_down_and_up() {
        assert_eq!(control(117, 127), Some(Event::Key { key: Key::Play, down: true }));
        assert_eq!(control(117, 0), Some(Event::Key { key: Key::Play, down: false }));
        assert_eq!(control(14, 127), Some(Event::Key { key: Key::BankRight, down: true }));
        assert_eq!(control(15, 127), Some(Event::Key { key: Key::BankLeft, down: true }));
        assert_eq!(control(52, 127), Some(Event::Key { key: Key::Select(3), down: true }));
        assert_eq!(control(77, 127), Some(Event::Key { key: Key::Device(3), down: true }));
        assert_eq!(control(60, 127), None, "an unmapped controller");
    }

    /// The same bytes on another channel, or a note, are not the surface.
    #[test]
    fn other_channels_and_notes_are_not_the_surface() {
        assert_eq!(parse(&[0xB0, 17, 1]), None);
        assert_eq!(parse(&[0xB1, 117, 127]), None);
        assert_eq!(parse(&[0x9E, 60, 100]), None);
        assert_eq!(parse(&[0xBE, 17]), None, "truncated");
        assert_eq!(parse(&[]), None);
    }

    /// Every key's controller number reads back as that key.
    #[test]
    fn keys_round_trip_through_their_controller() {
        let mut keys = vec![
            Key::Play, Key::Stop, Key::Record, Key::Loop, Key::Rewind, Key::FastForward,
            Key::BankLeft, Key::BankRight, Key::Peek, Key::Display, Key::Mute, Key::Solo,
            Key::Mixer, Key::SelectLeft, Key::SelectRight, Key::Mode,
        ];
        keys.extend((0..ENCODERS).map(Key::Select));
        keys.extend((0..display::PAGE_KEYS as u8).map(Key::Device));
        for key in keys {
            assert_eq!(key_of(key_cc(key)), Some(key), "{key:?}");
        }
    }

    /// The handshake bytes are the unit's, exactly.
    #[test]
    fn the_handshake_is_exact() {
        assert_eq!(inquiry(), [0xF0, 0x00, 0x01, 0x05, 0x20, 0x7F, 0x20, 0x2E, 0xF7]);
        assert_eq!(disconnect(), [0xF0, 0x00, 0x01, 0x05, 0x20, 0x7F, 0x20, 0x00, 0xF7]);
        assert_eq!(
            parse(&[0xF0, 0x00, 0x01, 0x05, 0x20, 0x7F, 0x20, 0x26, 0xF7]),
            Some(Incoming::Handshake { has_sliders: false })
        );
        assert_eq!(
            parse(&[0xF0, 0x00, 0x01, 0x05, 0x20, 0x7F, 0x20, 0x2E, 0xF7]),
            Some(Incoming::Handshake { has_sliders: true })
        );
        assert_eq!(parse(&[0xF0, 0x00, 0x01, 0x05, 0x20, 0x7F, 0x20, 0x2B, 0xF7]), None, "another reply");
        assert_eq!(parse(&[0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, 0x00, 0xF7]), None, "a Mackie header");
        assert_eq!(parse(&[0xF0, 0x00, 0x01, 0x05, 0x20, 0x7F, 0x20]), None, "no end byte");
        assert_eq!(parse(&[0xF0, 0x00, 0x01, 0x05, 0x20, 0x7F, 0xF7]), None, "no command");
    }

    /// A payload is cut to seven bits, whatever it held.
    #[test]
    fn a_payload_is_seven_bits() {
        let m = sysex(Command::SetDisplay, &[0xFF, 0x80, 0x41]);
        assert_eq!(m, [0xF0, 0x00, 0x01, 0x05, 0x20, 0x7F, 0x11, 0x7F, 0x00, 0x41, 0xF7]);
    }

    /// Transport keys have a lamp; the others have none.
    #[test]
    fn only_the_transport_has_lamps() {
        assert_eq!(lamp(Key::Play, true), Some([0xBE, 117, 127]));
        assert_eq!(lamp(Key::Stop, false), Some([0xBE, 116, 0]));
        assert_eq!(lamp(Key::BankLeft, true), None);
        assert_eq!(lamp(Key::Select(0), true), None);
        assert_eq!(display_power(true), [0xBE, 79, 127]);
    }

    /// The port is recognised under both names it has.
    #[test]
    fn the_port_is_known_by_its_name() {
        assert!(is_port("Axiom Pro HyperControl"));
        assert!(is_port("Axiom Pro 25:Axiom Pro 25 MIDI 2 24:1"));
        assert!(!is_port("Axiom Pro 25:Axiom Pro 25 MIDI 1 24:0"));
        assert!(!is_port("Midi Through Port-0"));
        assert!(!is_port("X-Touch MIDI 2"));
    }
}
