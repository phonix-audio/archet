/// dB -> linear, the exact form the compressor below was tuned against.
#[inline(always)]
pub fn db_to_linear(db: f32) -> f32 {
    (db * std::f32::consts::LN_10 / 20.0).exp()
}

fn linear_to_db(lin: f32) -> f32 { 20.0 * lin.max(1e-10).log10() }

pub struct RmsCompressor {
    rms_sq: f32, env_db: f32,
    atk_coef: f32, rel_coef: f32, rms_coef: f32,
}

impl RmsCompressor {
    fn time_coef(t: f32, sr: f32) -> f32 { (-1.0 / (t * sr)).exp() }

    pub fn new(sr: f32) -> Self {
        Self {
            rms_sq: 0.0, env_db: 0.0,
            atk_coef: Self::time_coef(0.010, sr),
            rel_coef: Self::time_coef(0.080, sr),
            rms_coef: Self::time_coef(0.010, sr),
        }
    }

    pub fn new_limiter(sr: f32) -> Self {
        let mut s = Self::new(sr);
        s.set_times(0.001, 0.050, sr);
        s
    }

    pub fn set_times(&mut self, atk_s: f32, rel_s: f32, sr: f32) {
        self.atk_coef = Self::time_coef(atk_s.max(1e-5), sr);
        self.rel_coef = Self::time_coef(rel_s.max(1e-5), sr);
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32, thr_db: f32, ratio: f32, knee_db: f32, ceiling_db: f32) -> f32 {
        self.rms_sq = self.rms_coef * self.rms_sq + (1.0 - self.rms_coef) * x * x;
        let level_db = linear_to_db(self.rms_sq.sqrt());
        let gr = Self::gain_reduction(level_db, thr_db, ratio, knee_db);
        if gr > self.env_db {
            self.env_db = self.atk_coef * self.env_db + (1.0 - self.atk_coef) * gr;
        } else {
            self.env_db = self.rel_coef * self.env_db + (1.0 - self.rel_coef) * gr;
        }
        let mut out = x * db_to_linear(-self.env_db);
        if ceiling_db.is_finite() {
            let ceil = db_to_linear(ceiling_db);
            out = out.clamp(-ceil, ceil);
        }
        out
    }

    #[inline(always)]
    fn gain_reduction(level_db: f32, thr_db: f32, ratio: f32, knee_db: f32) -> f32 {
        let diff = level_db - thr_db;
        let half = knee_db * 0.5;
        let slope = 1.0 - 1.0 / ratio;
        if diff <= -half { 0.0 }
        else if diff >= half { diff * slope }
        else { let x = diff + half; x * x / (2.0 * knee_db.max(1e-6)) * slope }
    }
}
