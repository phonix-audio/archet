//! Speaker layouts past stereo, and where a track sits among them.

use serde::{Deserialize, Serialize};

/// Speakers a layout can have at most.
pub const MAX_SPEAKERS: usize = 12;

/// The bed the mix is laid on.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SpeakerLayout {
    #[default]
    Stereo,
    Surround51,
    Surround71,
    /// 7.1 with four height speakers, the Atmos bed.
    Atmos714,
}

/// One speaker: its azimuth in degrees, positive to the right, its
/// elevation, its name, and whether it is the low-frequency channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speaker {
    pub az: f32,
    pub el: f32,
    pub name: &'static str,
    pub lfe: bool,
}

const fn spk(az: f32, el: f32, name: &'static str) -> Speaker { Speaker { az, el, name, lfe: false } }
const LFE: Speaker = Speaker { az: 0.0, el: 0.0, name: "LFE", lfe: true };

const STEREO: [Speaker; 2] = [spk(-30.0, 0.0, "L"), spk(30.0, 0.0, "R")];
const S51: [Speaker; 6] = [spk(-30.0, 0.0, "L"), spk(30.0, 0.0, "R"), spk(0.0, 0.0, "C"), LFE, spk(-110.0, 0.0, "Ls"), spk(110.0, 0.0, "Rs")];
const S71: [Speaker; 8] = [spk(-30.0, 0.0, "L"), spk(30.0, 0.0, "R"), spk(0.0, 0.0, "C"), LFE, spk(-90.0, 0.0, "Lss"), spk(90.0, 0.0, "Rss"), spk(-150.0, 0.0, "Lrs"), spk(150.0, 0.0, "Rrs")];
const S714: [Speaker; 12] = [spk(-30.0, 0.0, "L"), spk(30.0, 0.0, "R"), spk(0.0, 0.0, "C"), LFE, spk(-90.0, 0.0, "Lss"), spk(90.0, 0.0, "Rss"), spk(-150.0, 0.0, "Lrs"), spk(150.0, 0.0, "Rrs"),
    spk(-45.0, 45.0, "Ltf"), spk(45.0, 45.0, "Rtf"), spk(-135.0, 45.0, "Ltr"), spk(135.0, 45.0, "Rtr")];

impl SpeakerLayout {
    pub const ALL: [SpeakerLayout; 4] = [SpeakerLayout::Stereo, SpeakerLayout::Surround51, SpeakerLayout::Surround71, SpeakerLayout::Atmos714];

    pub fn name(self) -> &'static str {
        match self { SpeakerLayout::Stereo => "Stereo", SpeakerLayout::Surround51 => "5.1", SpeakerLayout::Surround71 => "7.1", SpeakerLayout::Atmos714 => "7.1.4" }
    }

    pub fn speakers(self) -> &'static [Speaker] {
        match self { SpeakerLayout::Stereo => &STEREO, SpeakerLayout::Surround51 => &S51, SpeakerLayout::Surround71 => &S71, SpeakerLayout::Atmos714 => &S714 }
    }

    pub fn channels(self) -> usize { self.speakers().len() }

    pub fn has_heights(self) -> bool { matches!(self, SpeakerLayout::Atmos714) }
}

/// Where a track sits: left to right, back to front, floor to ceiling,
/// each from -1 to 1; how wide it spreads, 0 to 1; and how much of it
/// goes to the low-frequency channel.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct SurroundPos {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub spread: f32,
    pub lfe: f32,
}

impl Default for SurroundPos {
    fn default() -> Self { Self { x: 0.0, y: 0.0, z: 0.0, spread: 0.5, lfe: 0.0 } }
}

fn unit(az_deg: f32, el_deg: f32) -> [f32; 3] {
    let (az, el) = (az_deg.to_radians(), el_deg.to_radians());
    [az.sin() * el.cos(), az.cos() * el.cos(), el.sin()]
}

/// The gains of one point source at azimuth `az` and elevation `el`
/// over the layout, for the speakers that are not the LFE: a cosine
/// law sharpened by `focus`, power normalized, blended with an even
/// spread by `1 - distance`.
fn point_gains(layout: SpeakerLayout, az: f32, el: f32, distance: f32, focus: f32) -> [f32; MAX_SPEAKERS] {
    let spk = layout.speakers();
    let src = unit(az, el);
    let mut g = [0.0f32; MAX_SPEAKERS];
    let n = spk.iter().filter(|s| !s.lfe).count().max(1) as f32;
    let mut power = 0.0f32;
    for (i, s) in spk.iter().enumerate() {
        if s.lfe { continue; }
        let d = unit(s.az, s.el);
        let c = (src[0] * d[0] + src[1] * d[1] + src[2] * d[2]).max(0.0);
        let directional = c.powf(focus);
        let even = 1.0 / n.sqrt();
        g[i] = directional * distance + even * (1.0 - distance);
        power += g[i] * g[i];
    }
    if power > 1e-9 {
        let k = 1.0 / power.sqrt();
        for v in &mut g { *v *= k; }
    }
    g
}

/// The gains a track's left and right channels take over the layout,
/// and the share of its mono sum for the LFE: the left channel sits a
/// little left of the position and the right a little right, by the
/// spread.
pub fn pan_gains(layout: SpeakerLayout, pos: &SurroundPos) -> ([f32; MAX_SPEAKERS], [f32; MAX_SPEAKERS], f32) {
    let z = if layout.has_heights() { pos.z.clamp(-1.0, 1.0) } else { 0.0 };
    let dist = (pos.x * pos.x + pos.y * pos.y + z * z).sqrt().min(1.0);
    let az = if dist < 1e-4 { 0.0 } else { pos.x.atan2(pos.y).to_degrees() };
    let el = if dist < 1e-4 { 0.0 } else { (z / dist).clamp(-1.0, 1.0).asin().to_degrees().max(0.0) };
    let spread = pos.spread.clamp(0.0, 1.0);
    let half_width = 30.0 * spread;
    let focus = 2.0 + 6.0 * (1.0 - spread);
    let left = point_gains(layout, az - half_width, el, dist, focus);
    let right = point_gains(layout, az + half_width, el, dist, focus);
    (left, right, pos.lfe.clamp(0.0, 1.0))
}

/// The stereo pair a multichannel frame folds down to: the front pair
/// as it is, the centre and the other speakers three decibels down by
/// their azimuth, the LFE six.
pub fn downmix(layout: SpeakerLayout, frame: &[f32]) -> (f32, f32) {
    let (mut l, mut r) = (0.0f32, 0.0f32);
    for (i, s) in layout.speakers().iter().enumerate() {
        let v = frame.get(i).copied().unwrap_or(0.0);
        if s.lfe { l += v * 0.5; r += v * 0.5; continue; }
        if s.el == 0.0 && (s.az - 30.0).abs() < 1e-3 { r += v; continue; }
        if s.el == 0.0 && (s.az + 30.0).abs() < 1e-3 { l += v; continue; }
        if s.el == 0.0 && s.az.abs() < 1e-3 {
            l += v * std::f32::consts::FRAC_1_SQRT_2;
            r += v * std::f32::consts::FRAC_1_SQRT_2;
            continue;
        }
        let side = s.az.to_radians().sin();
        let wl = ((1.0 - side) * 0.5).sqrt();
        let wr = ((1.0 + side) * 0.5).sqrt();
        l += v * wl * std::f32::consts::FRAC_1_SQRT_2;
        r += v * wr * std::f32::consts::FRAC_1_SQRT_2;
    }
    (l, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx(layout: SpeakerLayout, name: &str) -> usize {
        layout.speakers().iter().position(|s| s.name == name).unwrap()
    }

    #[test]
    fn a_source_goes_to_the_speaker_it_faces() {
        let l = SpeakerLayout::Surround51;
        let (gl, gr, lfe) = pan_gains(l, &SurroundPos { x: 0.0, y: 1.0, z: 0.0, spread: 0.0, lfe: 0.3 });
        let c = idx(l, "C");
        assert!(gl[c] > 0.9 && gr[c] > 0.9, "front centre: {:?}", &gl[..6]);
        assert!((lfe - 0.3).abs() < 1e-6);
        let (gl, _, _) = pan_gains(l, &SurroundPos { x: -1.0, y: -0.5, z: 0.0, spread: 0.0, lfe: 0.0 });
        let ls = idx(l, "Ls");
        assert!(gl[ls] > gl[idx(l, "L")] && gl[ls] > gl[c], "back left: {:?}", &gl[..6]);
        let power: f32 = gl.iter().map(|g| g * g).sum();
        assert!((power - 1.0).abs() < 1e-3, "power normalized: {power}");
        let (gl, _, _) = pan_gains(l, &SurroundPos::default());
        let spread_evenly = gl.iter().take(6).filter(|g| **g > 0.3).count();
        assert!(spread_evenly >= 5, "the centre of the room spreads everywhere: {:?}", &gl[..6]);
    }

    #[test]
    fn heights_only_count_on_the_atmos_bed() {
        let up = SurroundPos { x: 0.0, y: 0.5, z: 1.0, spread: 0.0, lfe: 0.0 };
        let (g, _, _) = pan_gains(SpeakerLayout::Atmos714, &up);
        let ltf = idx(SpeakerLayout::Atmos714, "Ltf");
        assert!(g[ltf] > g[idx(SpeakerLayout::Atmos714, "L")], "{:?}", &g[..12]);
        let (g, _, _) = pan_gains(SpeakerLayout::Surround71, &up);
        assert!(g[idx(SpeakerLayout::Surround71, "C")] > 0.5, "no heights: the source stays on the floor");
        assert_eq!(SpeakerLayout::Atmos714.channels(), 12);
    }

    #[test]
    fn a_downmix_keeps_left_on_the_left() {
        let l = SpeakerLayout::Surround51;
        let mut frame = [0.0f32; 6];
        frame[idx(l, "Ls")] = 1.0;
        let (a, b) = downmix(l, &frame);
        assert!(a > 0.5 && b < 0.2, "{a} {b}");
        let mut frame = [0.0f32; 6];
        frame[idx(l, "C")] = 1.0;
        let (a, b) = downmix(l, &frame);
        assert!((a - b).abs() < 1e-6 && a > 0.6);
    }
}
