//! Integer stereo delay for plugin delay compensation.

/// A fixed-length stereo delay: the output is the input `delay` frames
/// ago, and zero for the first `delay` frames after a change.
#[derive(Debug, Clone, Default)]
pub struct StereoDelay {
    buf: Vec<[f32; 2]>,
    pos: usize,
}

impl StereoDelay {
    pub fn new() -> Self { Self::default() }

    pub fn delay(&self) -> usize { self.buf.len() }

    /// Set the delay in frames. Reallocates and clears when it changes;
    /// unchanged delays keep their history.
    pub fn set_delay(&mut self, frames: usize) {
        if frames == self.buf.len() { return; }
        self.buf = vec![[0.0; 2]; frames];
        self.pos = 0;
    }

    pub fn reset(&mut self) {
        for f in &mut self.buf { *f = [0.0; 2]; }
        self.pos = 0;
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.buf.is_empty() { return (l, r); }
        let out = self.buf[self.pos];
        self.buf[self.pos] = [l, r];
        self.pos += 1;
        if self.pos == self.buf.len() { self.pos = 0; }
        (out[0], out[1])
    }

    /// Delay a block in place.
    pub fn process_block(&mut self, block: &mut [[f32; 2]]) {
        if self.buf.is_empty() { return; }
        for fr in block {
            let (l, r) = self.process(fr[0], fr[1]);
            *fr = [l, r];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_by_exactly_n_frames() {
        let mut d = StereoDelay::new();
        d.set_delay(3);
        let mut block: Vec<[f32; 2]> = (1..=8).map(|i| [i as f32, -(i as f32)]).collect();
        d.process_block(&mut block);
        assert_eq!(block[0], [0.0, 0.0]);
        assert_eq!(block[2], [0.0, 0.0]);
        assert_eq!(block[3], [1.0, -1.0]);
        assert_eq!(block[7], [5.0, -5.0]);
    }

    #[test]
    fn zero_delay_passes_through_and_change_clears() {
        let mut d = StereoDelay::new();
        assert_eq!(d.process(0.5, 0.25), (0.5, 0.25));
        d.set_delay(2);
        d.process(1.0, 1.0);
        d.set_delay(2);
        assert_eq!(d.process(0.0, 0.0), (0.0, 0.0));
        assert_eq!(d.process(0.0, 0.0), (1.0, 1.0));
        d.set_delay(1);
        assert_eq!(d.process(0.0, 0.0), (0.0, 0.0));
    }
}
