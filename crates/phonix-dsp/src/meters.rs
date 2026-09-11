//! Level meters: VU, visual segments, true-peak/RMS audio meter, integrated
//! and short-term LUFS, and a CPU load meter.

use std::time::{Duration, Instant};

//==============================================================================
// VU METER
//==============================================================================

/// VU Meter for measuring audio levels
#[derive(Debug, Clone)]
pub struct VuMeter {
    sample_rate: f32,

    // Current levels (linear, 0.0 to 1.0+)
    level_left: f32,
    level_right: f32,

    // Peak levels
    peak_left: f32,
    peak_right: f32,

    // Peak hold timers
    peak_hold_left: f32,
    peak_hold_right: f32,
    peak_hold_time: f32,  // seconds

    // Integration time for RMS calculation
    integration_time: f32,  // seconds
    // Interleaved L/R squared-sample circular buffer.
    rms_window: Vec<f32>,
    rms_window_pos: usize,
    // Running sums of `left_sq` / `right_sq` over the window. O(1)
    // update per sample (subtract evicted, add new) instead of
    // re-summing 14400 entries every sample like the original
    // implementation. Per-sample meter cost dropped from O(N) to
    // O(1) — biggest single win in the drum-engine inner loop.
    rms_sum_left: f32,
    rms_sum_right: f32,
    rms_window_count: usize,

    // Ballistics
    attack_time: f32,   // seconds
    release_time: f32,  // seconds
    // Cached ballistics coefficients. Recomputed only on sample-
    // rate / attack / release change instead of calling `exp()`
    // every sample.
    attack_coef: f32,
    release_coef: f32,
}

impl VuMeter {
    pub fn new(sample_rate: f32) -> Self {
        // 300 ms RMS window, sized in stereo pairs (left+right
        // interleaved). Storing the pair count + 2-stride window
        // keeps eviction unambiguous: position p evicts the pair at
        // [p, p+1] before overwriting it.
        let pair_count = (sample_rate * 0.3) as usize;
        let window_size = pair_count * 2;
        let attack_time = 0.01;
        let release_time = 0.3;
        let attack_coef = (-1.0 / (attack_time * sample_rate)).exp();
        let release_coef = (-1.0 / (release_time * sample_rate)).exp();

        Self {
            sample_rate,
            level_left: 0.0,
            level_right: 0.0,
            peak_left: 0.0,
            peak_right: 0.0,
            peak_hold_left: 0.0,
            peak_hold_right: 0.0,
            peak_hold_time: 1.5,  // 1.5 seconds hold
            integration_time: 0.3,
            rms_window: vec![0.0; window_size],
            rms_window_pos: 0,
            rms_sum_left: 0.0,
            rms_sum_right: 0.0,
            rms_window_count: pair_count.max(1),
            attack_time,
            release_time,
            attack_coef,
            release_coef,
        }
    }

    /// Process a stereo sample and update levels
    pub fn process(&mut self, left: f32, right: f32) {
        // Calculate squared values for RMS
        let left_sq = left * left;
        let right_sq = right * right;

        // Evict the oldest stereo pair from the running sum, then
        // overwrite with the new pair and add it back in.
        let pos = self.rms_window_pos;
        self.rms_sum_left  -= self.rms_window[pos];
        self.rms_sum_right -= self.rms_window[pos + 1];
        self.rms_window[pos]     = left_sq;
        self.rms_window[pos + 1] = right_sq;
        self.rms_sum_left  += left_sq;
        self.rms_sum_right += right_sq;
        let next = pos + 2;
        self.rms_window_pos = if next >= self.rms_window.len() { 0 } else { next };

        // Mean of squares — guard against negative due to FP drift.
        let inv_n = 1.0 / self.rms_window_count as f32;
        let rms_left  = (self.rms_sum_left  * inv_n).max(0.0).sqrt();
        let rms_right = (self.rms_sum_right * inv_n).max(0.0).sqrt();

        // Apply ballistics (smooth level changes) using cached coefs.
        let attack_coef  = self.attack_coef;
        let release_coef = self.release_coef;

        // Update left level
        if rms_left > self.level_left {
            self.level_left = attack_coef * (self.level_left - rms_left) + rms_left;
        } else {
            self.level_left = release_coef * (self.level_left - rms_left) + rms_left;
        }

        // Update right level
        if rms_right > self.level_right {
            self.level_right = attack_coef * (self.level_right - rms_right) + rms_right;
        } else {
            self.level_right = release_coef * (self.level_right - rms_right) + rms_right;
        }

        // Update peaks
        let abs_left = left.abs();
        let abs_right = right.abs();

        if abs_left > self.peak_left {
            self.peak_left = abs_left;
            self.peak_hold_left = self.peak_hold_time * self.sample_rate;
        } else if self.peak_hold_left > 0.0 {
            self.peak_hold_left -= 1.0;
            if self.peak_hold_left <= 0.0 {
                self.peak_left = self.peak_left * 0.99; // Slow decay
            }
        }

        if abs_right > self.peak_right {
            self.peak_right = abs_right;
            self.peak_hold_right = self.peak_hold_time * self.sample_rate;
        } else if self.peak_hold_right > 0.0 {
            self.peak_hold_right -= 1.0;
            if self.peak_hold_right <= 0.0 {
                self.peak_right = self.peak_right * 0.99;
            }
        }
    }
    
    /// Get current levels in dB
    pub fn get_levels_db(&self) -> (f32, f32) {
        let left_db = linear_to_db(self.level_left);
        let right_db = linear_to_db(self.level_right);
        (left_db, right_db)
    }
    
    /// Get current levels (linear, 0.0 to 1.0+)
    pub fn get_levels(&self) -> (f32, f32) {
        (self.level_left, self.level_right)
    }
    
    /// Get peak levels in dB
    pub fn get_peaks_db(&self) -> (f32, f32) {
        let left_db = linear_to_db(self.peak_left);
        let right_db = linear_to_db(self.peak_right);
        (left_db, right_db)
    }
    
    /// Get peak levels (linear)
    pub fn get_peaks(&self) -> (f32, f32) {
        (self.peak_left, self.peak_right)
    }
    
    /// Check if clipping (level > 0 dB)
    pub fn is_clipping(&self) -> bool {
        self.peak_left > 1.0 || self.peak_right > 1.0
    }
    
    /// Reset the meter
    pub fn reset(&mut self) {
        self.level_left = 0.0;
        self.level_right = 0.0;
        self.peak_left = 0.0;
        self.peak_right = 0.0;
        self.peak_hold_left = 0.0;
        self.peak_hold_right = 0.0;
        for sample in self.rms_window.iter_mut() {
            *sample = 0.0;
        }
        self.rms_window_pos = 0;
        self.rms_sum_left = 0.0;
        self.rms_sum_right = 0.0;
    }

    /// Set sample rate
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if (self.sample_rate - sample_rate).abs() > 1.0 {
            self.sample_rate = sample_rate;
            let pair_count = (sample_rate * self.integration_time) as usize;
            let window_size = pair_count * 2;
            self.rms_window = vec![0.0; window_size];
            self.rms_window_pos = 0;
            self.rms_sum_left = 0.0;
            self.rms_sum_right = 0.0;
            self.rms_window_count = pair_count.max(1);
            self.attack_coef  = (-1.0 / (self.attack_time  * sample_rate)).exp();
            self.release_coef = (-1.0 / (self.release_time * sample_rate)).exp();
        }
    }
}

/// Convert linear amplitude to dB
pub fn linear_to_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        -60.0  // Minimum
    } else {
        20.0 * linear.log10()
    }
}

/// Convert dB to linear amplitude
pub fn db_to_linear(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

//==============================================================================
// CPU USAGE METER
//==============================================================================

/// CPU usage meter for performance monitoring
#[derive(Debug, Clone)]
pub struct CpuUsageMeter {
    // Timing
    start_time: Option<Instant>,
    total_processing_time: Duration,
    sample_count: u64,
    
    // Sample rate for calculating available time
    sample_rate: f32,
    buffer_size: usize,
    
    // Rolling average for smooth display
    usage_history: Vec<f32>,
    history_pos: usize,
    history_size: usize,
    
    // Current values
    current_usage: f32,
    peak_usage: f32,
}

impl CpuUsageMeter {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            start_time: None,
            total_processing_time: Duration::ZERO,
            sample_count: 0,
            sample_rate,
            buffer_size: 256,
            usage_history: vec![0.0; 60],  // 60 samples for 1 second at 60fps
            history_pos: 0,
            history_size: 60,
            current_usage: 0.0,
            peak_usage: 0.0,
        }
    }
    
    /// Start timing a processing block
    pub fn start(&mut self) {
        self.start_time = Some(Instant::now());
    }
    
    /// Stop timing and record the processing time
    pub fn stop(&mut self) {
        if let Some(start) = self.start_time {
            let elapsed = start.elapsed();
            self.total_processing_time += elapsed;
            self.sample_count += 1;
            self.start_time = None;
            
            // Update usage every buffer
            if self.sample_count % self.buffer_size as u64 == 0 {
                self.update_usage();
            }
        }
    }
    
    /// Calculate and update current CPU usage
    fn update_usage(&mut self) {
        if self.sample_count == 0 {
            return;
        }
        
        // Calculate available time per buffer
        let available_time = Duration::from_secs_f32(
            self.buffer_size as f32 / self.sample_rate
        );
        
        // Calculate usage percentage
        let usage = if available_time.as_nanos() > 0 {
            let avg_time_per_buffer = self.total_processing_time.as_nanos() as f64 
                / (self.sample_count as f64 / self.buffer_size as f64);
            
            (avg_time_per_buffer / available_time.as_nanos() as f64 * 100.0) as f32
        } else {
            0.0
        };
        
        // Clamp and store
        self.current_usage = usage.min(100.0).max(0.0);
        
        // Update peak
        if self.current_usage > self.peak_usage {
            self.peak_usage = self.current_usage;
        }
        
        // Update history for rolling average
        self.usage_history[self.history_pos] = self.current_usage;
        self.history_pos = (self.history_pos + 1) % self.history_size;
    }
    
    /// Get current CPU usage percentage
    pub fn usage(&self) -> f32 {
        self.current_usage
    }
    
    /// Get average CPU usage over history
    pub fn average_usage(&self) -> f32 {
        if self.usage_history.is_empty() {
            return 0.0;
        }
        
        let sum: f32 = self.usage_history.iter().sum();
        sum / self.usage_history.len() as f32
    }
    
    /// Get peak CPU usage
    pub fn peak_usage(&self) -> f32 {
        self.peak_usage
    }
    
    /// Reset peak usage
    pub fn reset_peak(&mut self) {
        self.peak_usage = 0.0;
    }
    
    /// Reset the meter completely
    pub fn reset(&mut self) {
        self.start_time = None;
        self.total_processing_time = Duration::ZERO;
        self.sample_count = 0;
        self.current_usage = 0.0;
        self.peak_usage = 0.0;
        for val in self.usage_history.iter_mut() {
            *val = 0.0;
        }
        self.history_pos = 0;
    }
    
    /// Set sample rate
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }
    
    /// Set buffer size
    pub fn set_buffer_size(&mut self, buffer_size: usize) {
        self.buffer_size = buffer_size;
    }
}

//==============================================================================
// STEREO LEVEL METER (Visual)
//==============================================================================

/// Level meter segment for visual display
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterSegment {
    Green,   // -60 to -12 dB
    Yellow,  // -12 to -6 dB
    Orange,  // -6 to 0 dB
    Red,     // 0 dB and above (clipping)
    Off,
}

/// Visual meter representation
#[derive(Debug, Clone)]
pub struct VisualMeter {
    // Segments count for each color
    green_segments: usize,
    yellow_segments: usize,
    orange_segments: usize,
    red_segments: usize,
    
    // Total segments
    total_segments: usize,
}

impl VisualMeter {
    pub fn new() -> Self {
        Self {
            green_segments: 8,   // -60 to -12 dB
            yellow_segments: 3,  // -12 to -6 dB
            orange_segments: 3,  // -6 to 0 dB
            red_segments: 2,     // 0 dB and above
            total_segments: 16,
        }
    }
    
    /// Convert dB level to segment display
    pub fn db_to_segments(&self, db: f32) -> Vec<MeterSegment> {
        let mut segments = vec![MeterSegment::Off; self.total_segments];
        
        // Map dB to segment index
        let segment_index = self.db_to_index(db);
        
        // Fill segments up to the current level
        for i in 0..segment_index.min(self.total_segments) {
            segments[i] = self.index_to_segment(i);
        }
        
        segments
    }
    
    /// Convert dB to segment index
    fn db_to_index(&self, db: f32) -> usize {
        // -60 dB = 0, 0 dB = 14, +6 dB = 16
        if db < -60.0 {
            return 0;
        }
        
        // Scale: each green segment = 6 dB, yellow = 2 dB, orange = 2 dB, red = 3 dB
        if db < -12.0 {
            // Green zone: -60 to -12 dB (8 segments)
            let db_range = 48.0;  // -60 to -12
            let normalized = (db + 60.0) / db_range;
            (normalized * self.green_segments as f32) as usize
        } else if db < -6.0 {
            // Yellow zone: -12 to -6 dB (3 segments)
            let db_range = 6.0;
            let normalized = (db + 12.0) / db_range;
            self.green_segments + (normalized * self.yellow_segments as f32) as usize
        } else if db < 0.0 {
            // Orange zone: -6 to 0 dB (3 segments)
            let db_range = 6.0;
            let normalized = (db + 6.0) / db_range;
            self.green_segments + self.yellow_segments + (normalized * self.orange_segments as f32) as usize
        } else {
            // Red zone: 0 dB and above (2 segments)
            let db_range = 6.0;
            let normalized = (db / db_range).min(1.0);
            self.green_segments + self.yellow_segments + self.orange_segments 
                + (normalized * self.red_segments as f32) as usize
        }
    }
    
    /// Convert segment index to color
    fn index_to_segment(&self, index: usize) -> MeterSegment {
        if index < self.green_segments {
            MeterSegment::Green
        } else if index < self.green_segments + self.yellow_segments {
            MeterSegment::Yellow
        } else if index < self.green_segments + self.yellow_segments + self.orange_segments {
            MeterSegment::Orange
        } else {
            MeterSegment::Red
        }
    }
    
    /// Get total segments
    pub fn total_segments(&self) -> usize {
        self.total_segments
    }
}

impl Default for VisualMeter {
    fn default() -> Self {
        Self::new()
    }
}

//==============================================================================
// COMBINED AUDIO METER
//==============================================================================

/// Combined meter with VU, peak, and CPU display
#[derive(Debug, Clone)]
pub struct AudioMeter {
    vu: VuMeter,
    cpu: CpuUsageMeter,
    visual: VisualMeter,
}

impl AudioMeter {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            vu: VuMeter::new(sample_rate),
            cpu: CpuUsageMeter::new(sample_rate),
            visual: VisualMeter::new(),
        }
    }
    
    /// Process stereo sample
    pub fn process(&mut self, left: f32, right: f32) {
        self.vu.process(left, right);
    }
    
    /// Start CPU timing
    pub fn start_cpu(&mut self) {
        self.cpu.start();
    }
    
    /// Stop CPU timing
    pub fn stop_cpu(&mut self) {
        self.cpu.stop();
    }
    
    /// Get VU levels in dB
    pub fn vu_levels_db(&self) -> (f32, f32) {
        self.vu.get_levels_db()
    }
    
    /// Get peak levels in dB
    pub fn peak_levels_db(&self) -> (f32, f32) {
        self.vu.get_peaks_db()
    }
    
    /// Get CPU usage
    pub fn cpu_usage(&self) -> f32 {
        self.cpu.usage()
    }
    
    /// Get visual segments for left channel
    pub fn visual_left(&self) -> Vec<MeterSegment> {
        let (left_db, _) = self.vu.get_levels_db();
        self.visual.db_to_segments(left_db)
    }
    
    /// Get visual segments for right channel
    pub fn visual_right(&self) -> Vec<MeterSegment> {
        let (_, right_db) = self.vu.get_levels_db();
        self.visual.db_to_segments(right_db)
    }
    
    /// Get visual segments for peaks
    pub fn visual_peaks(&self) -> (Vec<MeterSegment>, Vec<MeterSegment>) {
        let (left_db, right_db) = self.vu.get_peaks_db();
        (
            self.visual.db_to_segments(left_db),
            self.visual.db_to_segments(right_db),
        )
    }
    
    /// Check if clipping
    pub fn is_clipping(&self) -> bool {
        self.vu.is_clipping()
    }
    
    /// Reset all meters
    pub fn reset(&mut self) {
        self.vu.reset();
        self.cpu.reset();
    }
    
    /// Set sample rate
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.vu.set_sample_rate(sample_rate);
        self.cpu.set_sample_rate(sample_rate);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LUFS METER (ITU-R BS.1770-4)
// ─────────────────────────────────────────────────────────────────────────────

/// K-weighting biquad for ITU-R BS.1770 loudness measurement.
/// Implemented as a 2nd-order direct-form-I filter so we can plug in
/// either the pre-filter (high shelf at ~1.5 kHz, +4 dB) or the
/// revised high-pass (38 Hz Butterworth) coefficients.
///
/// Coefficients are derived per sample rate via bilinear transform of
/// the analog prototype the standard specifies, so the meter stays
/// accurate at 44.1 / 48 / 96 kHz without hand-tuned per-rate tables.
#[derive(Debug, Clone, Copy)]
struct KBiquad {
    b0: f32, b1: f32, b2: f32,
    a1: f32, a2: f32,
    x1: f32, x2: f32, y1: f32, y2: f32,
}

impl KBiquad {
    fn new() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0,
               x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }

    /// Configure as the BS.1770 pre-filter (head-related high shelf):
    /// f0 ~1681 Hz, +4 dB, Q = 0.7071. Coefficients come from a
    /// bilinear-transform of the analog prototype the spec gives.
    fn set_pre_filter(&mut self, sr: f32) {
        let f0 = 1681.974450955533_f64;
        let g  = 3.999843853973347_f64;
        let q  = 0.7071752369554196_f64;
        let k  = (std::f64::consts::PI * f0 / sr as f64).tan();
        let vh = 10.0_f64.powf(g / 20.0);
        let vb = vh.powf(0.499666774155);
        let a0 = 1.0 + k / q + k * k;
        self.b0 = ((vh + vb * k / q + k * k) / a0) as f32;
        self.b1 = (2.0 * (k * k - vh) / a0) as f32;
        self.b2 = ((vh - vb * k / q + k * k) / a0) as f32;
        self.a1 = (2.0 * (k * k - 1.0) / a0) as f32;
        self.a2 = ((1.0 - k / q + k * k) / a0) as f32;
        self.reset();
    }

    /// Configure as the BS.1770 revised high-pass (38 Hz, Q = 0.5).
    fn set_high_pass(&mut self, sr: f32) {
        let f0 = 38.13547087602444_f64;
        let q  = 0.5003270373238773_f64;
        let k  = (std::f64::consts::PI * f0 / sr as f64).tan();
        let a0 = 1.0 + k / q + k * k;
        self.b0 = (1.0 / a0) as f32;
        self.b1 = (-2.0 / a0) as f32;
        self.b2 = (1.0 / a0) as f32;
        self.a1 = (2.0 * (k * k - 1.0) / a0) as f32;
        self.a2 = ((1.0 - k / q + k * k) / a0) as f32;
        self.reset();
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
              - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1; self.x1 = x;
        self.y2 = self.y1; self.y1 = y;
        y
    }

    fn reset(&mut self) {
        self.x1 = 0.0; self.x2 = 0.0; self.y1 = 0.0; self.y2 = 0.0;
    }
}

/// ITU-R BS.1770-4 loudness meter with momentary (400 ms),
/// short-term (3 s) and gated integrated values. Stereo input;
/// each channel runs an independent K-weighted square accumulator
/// and the per-channel mean-squares are summed (channel weights
/// 1.0 / 1.0 for stereo, as the spec specifies).
///
/// The momentary and short-term windows are sliding rectangular
/// averages over the K-weighted mean square. Integrated loudness
/// follows the spec's two-pass gating:
///   1. Drop 400 ms blocks below -70 LUFS (absolute gate).
///   2. Drop blocks below (mean of step-1 blocks) - 10 LU
///      (relative gate).
///   3. Integrated = mean of remaining blocks, in LUFS.
/// We retain at most `MAX_BLOCKS` 400 ms blocks (~30 min at 75 %
/// overlap) so the buffer stays bounded.
#[derive(Debug, Clone)]
pub struct LufsMeter {
    // K-weighting filters, one pair per channel.
    pre_l: KBiquad, pre_r: KBiquad,
    hp_l:  KBiquad, hp_r:  KBiquad,

    // Running mean-square buffers for momentary (400 ms) and
    // short-term (3 s) windows. We use a circular buffer of squared
    // samples + a running sum so updates are O(1) per sample.
    moment_buf:    Vec<f32>,
    moment_sum:    f64,
    moment_pos:    usize,
    moment_window: usize,

    short_buf:    Vec<f32>,
    short_sum:    f64,
    short_pos:    usize,
    short_window: usize,

    // Block accumulator for integrated loudness — 400 ms hop with
    // 75 % overlap (so a new block boundary every 100 ms). We push
    // the block's mean-square into `blocks` and recompute the gated
    // mean on demand via `integrated_lufs`.
    block_buf:     Vec<f32>,
    block_sum:     f64,
    block_pos:     usize,
    block_samples: usize,
    block_hop:     usize,
    block_hop_pos: usize,
    blocks:        Vec<f32>,        // per-block mean-square (linear)
    max_blocks:    usize,
}

impl LufsMeter {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let mut pre_l = KBiquad::new(); pre_l.set_pre_filter(sr);
        let mut pre_r = KBiquad::new(); pre_r.set_pre_filter(sr);
        let mut hp_l  = KBiquad::new(); hp_l.set_high_pass(sr);
        let mut hp_r  = KBiquad::new(); hp_r.set_high_pass(sr);
        let moment_window = (sr * 0.4) as usize;   // 400 ms
        let short_window  = (sr * 3.0) as usize;   // 3 s
        let block_samples = (sr * 0.4) as usize;   // 400 ms
        let block_hop     = (sr * 0.1) as usize;   // 100 ms hop (75 % overlap)
        Self {
            pre_l, pre_r, hp_l, hp_r,
            moment_buf:    vec![0.0; moment_window],
            moment_sum:    0.0,
            moment_pos:    0,
            moment_window,
            short_buf:    vec![0.0; short_window],
            short_sum:    0.0,
            short_pos:    0,
            short_window,
            block_buf:     vec![0.0; block_samples],
            block_sum:     0.0,
            block_pos:     0,
            block_samples,
            block_hop,
            block_hop_pos: 0,
            blocks:        Vec::with_capacity(4096),
            // ~30 min of 100 ms hops — bounded; old entries drop off
            // the front when we exceed.
            max_blocks:    18_000,
        }
    }

    /// Process one stereo sample. Cheap per-sample work: 4 biquad
    /// updates + 3 ring-buffer subtract-add-stores + a block-boundary
    /// check.
    #[inline]
    pub fn process(&mut self, left: f32, right: f32) {
        // K-weight each channel — pre-filter then HP.
        let kl = self.hp_l.process(self.pre_l.process(left));
        let kr = self.hp_r.process(self.pre_r.process(right));
        // Channel-summed square. Stereo channel weights = 1.0 per
        // BS.1770; surround formats would scale Ls/Rs by 1.41.
        let s = (kl * kl + kr * kr) as f64;

        // Momentary window — O(1) circular update.
        let prev = self.moment_buf[self.moment_pos] as f64;
        self.moment_sum += s - prev;
        if self.moment_sum < 0.0 { self.moment_sum = 0.0; }
        self.moment_buf[self.moment_pos] = s as f32;
        self.moment_pos = (self.moment_pos + 1) % self.moment_window;

        // Short-term window.
        let prev = self.short_buf[self.short_pos] as f64;
        self.short_sum += s - prev;
        if self.short_sum < 0.0 { self.short_sum = 0.0; }
        self.short_buf[self.short_pos] = s as f32;
        self.short_pos = (self.short_pos + 1) % self.short_window;

        // Block accumulator — independent 400 ms square sum that
        // closes a block every `block_hop` samples (100 ms hop = 75 %
        // overlap with the previous block).
        let prev = self.block_buf[self.block_pos] as f64;
        self.block_sum += s - prev;
        if self.block_sum < 0.0 { self.block_sum = 0.0; }
        self.block_buf[self.block_pos] = s as f32;
        self.block_pos = (self.block_pos + 1) % self.block_samples;
        self.block_hop_pos += 1;
        if self.block_hop_pos >= self.block_hop {
            self.block_hop_pos = 0;
            let mean_sq = (self.block_sum / self.block_samples as f64) as f32;
            self.blocks.push(mean_sq);
            if self.blocks.len() > self.max_blocks {
                // Drop the oldest tenth in one shot so the move
                // amortises (Vec::remove(0) per push would be O(N)).
                let drop_n = self.max_blocks / 10;
                self.blocks.drain(0..drop_n);
            }
        }
    }

    /// Convert a linear mean-square to LUFS per BS.1770:
    /// `LUFS = -0.691 + 10 * log10(MS)`. Returns `-inf`-ish when MS
    /// is at the silence floor (caller decides how to display).
    #[inline]
    fn ms_to_lufs(ms: f32) -> f32 {
        if ms <= 1.0e-12 { return -120.0; }
        -0.691 + 10.0 * ms.log10()
    }

    /// Momentary loudness — 400 ms sliding window. Tracks transients.
    pub fn momentary_lufs(&self) -> f32 {
        let ms = (self.moment_sum / self.moment_window.max(1) as f64) as f32;
        Self::ms_to_lufs(ms)
    }

    /// Short-term loudness — 3 s sliding window. Matches the
    /// "short-term" reading on professional loudness meters.
    pub fn short_term_lufs(&self) -> f32 {
        let ms = (self.short_sum / self.short_window.max(1) as f64) as f32;
        Self::ms_to_lufs(ms)
    }

    /// Gated integrated loudness per BS.1770-4. Computes the
    /// absolute-gated mean (drop blocks below -70 LUFS), derives the
    /// relative gate (mean - 10 LU), drops below-relative blocks,
    /// and returns the mean of the remainder.
    pub fn integrated_lufs(&self) -> f32 {
        if self.blocks.is_empty() { return -120.0; }
        // Absolute gate at -70 LUFS in mean-square space:
        //   LUFS_abs = -70 -> MS_abs = 10^((LUFS+0.691)/10)
        let ms_abs = 10.0_f32.powf((-70.0 + 0.691) / 10.0);
        let mut sum: f64 = 0.0;
        let mut count: usize = 0;
        for &b in &self.blocks {
            if b > ms_abs { sum += b as f64; count += 1; }
        }
        if count == 0 { return -120.0; }
        let mean_ms_abs = (sum / count as f64) as f32;
        let mean_lufs = Self::ms_to_lufs(mean_ms_abs);
        let ms_rel = 10.0_f32.powf((mean_lufs - 10.0 + 0.691) / 10.0);
        let mut sum2: f64 = 0.0;
        let mut count2: usize = 0;
        for &b in &self.blocks {
            if b > ms_abs && b > ms_rel { sum2 += b as f64; count2 += 1; }
        }
        if count2 == 0 { return mean_lufs; }
        let mean_ms = (sum2 / count2 as f64) as f32;
        Self::ms_to_lufs(mean_ms)
    }

    /// Clear all running state. Used on patch reload / loop wrap.
    pub fn reset(&mut self) {
        self.pre_l.reset(); self.pre_r.reset();
        self.hp_l.reset();  self.hp_r.reset();
        for v in &mut self.moment_buf { *v = 0.0; } self.moment_sum = 0.0; self.moment_pos = 0;
        for v in &mut self.short_buf  { *v = 0.0; } self.short_sum  = 0.0; self.short_pos  = 0;
        for v in &mut self.block_buf  { *v = 0.0; } self.block_sum  = 0.0; self.block_pos  = 0;
        self.block_hop_pos = 0;
        self.blocks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vu_meter() {
        let mut meter = VuMeter::new(44100.0);
        
        // Process some samples
        for _ in 0..1000 {
            meter.process(0.5, 0.5);
        }
        
        let (left, right) = meter.get_levels();
        assert!(left > 0.0);
        assert!(right > 0.0);
    }
    
    #[test]
    fn test_linear_to_db() {
        assert!((linear_to_db(1.0) - 0.0).abs() < 0.001);
        assert!((linear_to_db(0.5) - (-6.02)).abs() < 0.1);
        assert!((linear_to_db(0.1) - (-20.0)).abs() < 0.1);
        assert!(linear_to_db(0.0) < -50.0);
    }
    
    #[test]
    fn test_db_to_linear() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 0.001);
        assert!((db_to_linear(-6.0) - 0.501).abs() < 0.01);
    }
    
    #[test]
    fn test_visual_meter() {
        let meter = VisualMeter::new();
        
        // Test -inf dB (silence)
        let segments = meter.db_to_segments(-100.0);
        assert!(segments.iter().all(|&s| s == MeterSegment::Off));
        
        // Test 0 dB
        let segments = meter.db_to_segments(0.0);
        assert!(segments.iter().take(14).all(|&s| s != MeterSegment::Off));
        
        // Test clipping
        let segments = meter.db_to_segments(3.0);
        assert!(segments.iter().any(|&s| s == MeterSegment::Red));
    }
    
    #[test]
    fn lufs_meter_calibration_minus_23_sine() {
        // BS.1770 calibration anchor: a 1 kHz sine at -20 dBFS through
        // a single channel (silence on the other) measures exactly
        // -23.0 LUFS. With both channels carrying identical -20 dBFS
        // the sum-of-channels formula yields +3 dB, i.e. -20 LUFS.
        // We test the canonical single-channel form so it's
        // unambiguous when the reading drifts from the spec.
        let sr = 48_000.0_f32;
        let mut m = LufsMeter::new(sr);
        let amp = 10.0_f32.powf(-20.0 / 20.0); // -20 dBFS peak ≈ 0.1
        let mut phase = 0.0_f32;
        let inc = 1000.0 / sr;
        // 4 seconds of audio — covers the 3 s short-term window plus
        // enough block hops for the integrated-loudness gate to
        // stabilise.
        for _ in 0..(sr as usize * 4) {
            let s = (phase * std::f32::consts::TAU).sin() * amp;
            phase += inc;
            if phase >= 1.0 { phase -= 1.0; }
            m.process(s, 0.0); // single-channel signal per BS.1770
        }
        let m_lufs = m.momentary_lufs();
        let s_lufs = m.short_term_lufs();
        let i_lufs = m.integrated_lufs();
        // -20 dBFS 1 kHz sine measured -> ~-23.0 LUFS (BS.1770
        // calibration anchor). Allow ±0.6 LU for K-weighting at
        // 1 kHz (K-curve has ~+1 dB gain at 1 kHz from the high
        // shelf, partially offset by the HP).
        assert!((m_lufs - (-23.0)).abs() < 0.6,
            "momentary LUFS for -20 dBFS sine should ~= -23; got {m_lufs}");
        assert!((s_lufs - (-23.0)).abs() < 0.6,
            "short-term LUFS for -20 dBFS sine should ~= -23; got {s_lufs}");
        assert!((i_lufs - (-23.0)).abs() < 0.6,
            "integrated LUFS for -20 dBFS sine should ~= -23; got {i_lufs}");
    }

    #[test]
    fn lufs_meter_silence_reads_floor() {
        let mut m = LufsMeter::new(48_000.0);
        for _ in 0..96_000 { m.process(0.0, 0.0); }
        assert!(m.momentary_lufs() < -100.0,
            "silence should hit the -120 floor, got {}", m.momentary_lufs());
        assert!(m.integrated_lufs() < -100.0);
    }

    #[test]
    fn test_cpu_meter() {
        let mut meter = CpuUsageMeter::new(44100.0);
        meter.set_buffer_size(256);
        
        // Simulate some processing
        meter.start();
        std::thread::sleep(std::time::Duration::from_micros(100));
        meter.stop();
        
        // Usage should be calculated
        // Note: We can't assert a specific value, just that it works
    }
}