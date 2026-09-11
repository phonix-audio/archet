//! Pitch-tracked looped sample playback.
//!
//! v4 Phase 2. Each layer owns one `SamplePlayer` that pulls from the
//! global `samples::SampleLib`. Pitch tracking: playback_rate =
//! note_freq / asset.base_freq_hz, then scaled by the engine sample
//! rate ratio. Looping: when the fractional read position passes
//! `loop_end`, wrap back to `loop_start` (the asset bakes a
//! crossfade around the seam so no click).
//!
//! Stereo: the buffer is mono on disk; the player outputs mono and
//! the calling layer is responsible for pan / spread.

use super::samples::{shared as samples_shared, SampleAsset};

pub struct SamplePlayer {
    /// Engine sample rate.
    sr:        f32,
    /// Active sample id (flat index into the global lib). u16 is plenty.
    sample_id: u16,
    /// Fractional read position in source samples.
    pos:       f64,
    /// Cached playback rate (source samples per engine sample).
    rate:      f64,
    /// Cached frequency the rate was computed for; recompute when
    /// it changes.
    cached_freq: f32,
}

impl Default for SamplePlayer {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl Clone for SamplePlayer {
    fn clone(&self) -> Self {
        Self {
            sr:          self.sr,
            sample_id:   self.sample_id,
            pos:         self.pos,
            rate:        self.rate,
            cached_freq: self.cached_freq,
        }
    }
}

impl SamplePlayer {
    pub fn new(sr: f32) -> Self {
        Self {
            sr, sample_id: 0, pos: 0.0, rate: 1.0, cached_freq: 0.0,
        }
    }

    pub fn reset_state(&mut self) {
        self.pos = 0.0;
        self.cached_freq = 0.0;
    }

    /// Note-on: rewind to the asset's start (so attack is heard from
    /// the head) and update playback rate for the note frequency.
    pub fn note_on(&mut self, sample_id: u16, freq_hz: f32) {
        let lib = samples_shared();
        let n = lib.assets.len().max(1);
        self.sample_id = (sample_id as usize % n) as u16;
        self.pos = 0.0;
        self.update_rate(freq_hz);
    }

    fn update_rate(&mut self, freq_hz: f32) {
        let lib = samples_shared();
        let Some(asset) = lib.get(self.sample_id as usize) else {
            self.rate = 1.0;
            return;
        };
        if freq_hz < 1.0 || asset.base_freq_hz < 1.0 {
            self.rate = 1.0;
        } else {
            // Both effects: pitch ratio + sample-rate ratio.
            self.rate = (freq_hz as f64 / asset.base_freq_hz as f64)
                      * (asset.sample_rate as f64 / self.sr as f64);
        }
        self.cached_freq = freq_hz;
    }

    /// Set the active sample id without retriggering.
    pub fn set_sample(&mut self, sample_id: u16) {
        let lib = samples_shared();
        let n = lib.assets.len().max(1);
        let new_id = (sample_id as usize % n) as u16;
        if new_id != self.sample_id {
            self.sample_id = new_id;
            // Force rate recompute; the asset's base_freq may differ.
            let f = self.cached_freq;
            self.update_rate(f.max(1.0));
            // Clamp pos to new asset's bounds.
            let asset_len = lib.get(self.sample_id as usize)
                .map(|a| a.samples.len()).unwrap_or(0);
            if self.pos as usize >= asset_len {
                self.pos = 0.0;
            }
        }
    }

    /// Render one sample. Returns 0.0 if no library asset is loaded.
    /// Caller is responsible for amp envelope / pan.
    #[inline(always)]
    pub fn process(&mut self, freq_hz: f32, sample_id: u16) -> f32 {
        // Cheap dispatch: re-fetch the asset every call (it's a Vec
        // ref so just an indirection — no allocation).
        let lib = samples_shared();
        let n = lib.assets.len().max(1);
        let id = (sample_id as usize % n) as u16;
        if id != self.sample_id {
            self.set_sample(id);
        }
        if (freq_hz - self.cached_freq).abs() > 1e-3 {
            self.update_rate(freq_hz);
        }
        let asset = match lib.get(self.sample_id as usize) {
            Some(a) => a,
            None => return 0.0,
        };
        // Runtime loop crossfade: when the playhead enters the last
        // FADE_LEN samples before loop_end, blend in the same offset
        // past loop_start so the wrap is glitch-free regardless of
        // source content.
        const FADE_LEN: usize = 1024;
        let s = if (self.pos as usize) + FADE_LEN >= asset.loop_end
            && asset.loop_end > asset.loop_start + FADE_LEN
        {
            let into_fade = asset.loop_end as f64 - self.pos;
            // into_fade decreases from FADE_LEN down to 0 as we
            // approach loop_end.
            let t = (1.0 - (into_fade / FADE_LEN as f64).clamp(0.0, 1.0)) as f32;
            let tail_g = (1.0 - t).sqrt();
            let head_g = t.sqrt();
            let tail = read_interp(asset, self.pos);
            // Mirror position into the head crossfade region.
            let head_pos = asset.loop_start as f64 + (FADE_LEN as f64 - into_fade);
            let head = read_interp(asset, head_pos);
            tail * tail_g + head * head_g
        } else {
            read_interp(asset, self.pos)
        };
        // Advance with loop wrap.
        self.pos += self.rate;
        if (self.pos as usize) >= asset.loop_end {
            // Wrap inside the looped region. Preserves fractional
            // position so pitch stays exact across the wrap.
            let over = self.pos - asset.loop_end as f64;
            self.pos = asset.loop_start as f64 + over;
        }
        s
    }

    /// Read the current playhead without advancing (used by granular
    /// engine to scrub the buffer at arbitrary positions).
    pub fn peek_at(&self, pos: f64) -> f32 {
        let lib = samples_shared();
        match lib.get(self.sample_id as usize) {
            Some(a) => read_interp(a, pos),
            None => 0.0,
        }
    }

    /// Current asset (for granular access to the underlying buffer
    /// and metadata).
    pub fn current_asset_id(&self) -> u16 { self.sample_id }
}

#[inline(always)]
fn read_interp(asset: &SampleAsset, pos: f64) -> f32 {
    let n = asset.samples.len();
    if n == 0 { return 0.0; }
    let i0 = pos as usize % n;
    let i1 = (i0 + 1) % n;
    let frac = (pos - i0 as f64) as f32;
    let a = asset.samples[i0];
    let b = asset.samples[i1];
    a + (b - a) * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_on_then_render_produces_audio() {
        let mut p = SamplePlayer::new(48_000.0);
        p.note_on(0, 220.0);
        let mut peak = 0.0_f32;
        for _ in 0..2048 {
            let s = p.process(220.0, 0).abs();
            if s > peak { peak = s; }
        }
        assert!(peak > 0.05, "sample player rendered silence (peak {peak})");
    }

    #[test]
    fn pitch_shift_changes_rate() {
        let mut p = SamplePlayer::new(48_000.0);
        p.note_on(0, 220.0);
        let r1 = p.rate;
        let _ = p.process(440.0, 0);
        let r2 = p.rate;
        assert!(r2 > r1, "playing one octave up should increase rate (was {r1}, became {r2})");
    }

    #[test]
    fn loop_wrap_does_not_panic() {
        // Render past the loop end multiple times.
        let mut p = SamplePlayer::new(48_000.0);
        p.note_on(0, 110.0);
        for _ in 0..200_000 {
            let _ = p.process(110.0, 0);
        }
    }

    #[test]
    fn switching_sample_resets_safely() {
        let mut p = SamplePlayer::new(48_000.0);
        p.note_on(0, 220.0);
        for _ in 0..1024 { let _ = p.process(220.0, 0); }
        let _ = p.process(220.0, 5);  // mid-render switch
    }
}
