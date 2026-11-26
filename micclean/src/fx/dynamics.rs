// Compressor: Giannoulis, Massberg, Reiss, "Digital Dynamic Range Compressor Design" (JAES 2012),
// computador de ganho em dB com knee suave e detector de pico suave desacoplado.
// De-esser: no estilo do Calf Deesser (modo split): detecção nos agudos, redução só nos agudos.
use super::biquad::{BqParams, BqType, Biquad, Coefs, Width};
use crate::SR;
use serde::{Deserialize, Serialize};

const DB_TO_LN: f64 = std::f64::consts::LN_10 / 20.0;
const LN_TO_DB: f64 = 20.0 / std::f64::consts::LN_10;

fn coef(ms: f64) -> f64 {
    (-1.0 / (ms.max(0.05) / 1000.0 * SR as f64)).exp()
}

fn to_db(x: f64) -> f64 {
    (x.abs() + 1e-10).ln() * LN_TO_DB
}

fn flush_f64(s: &mut f64) {
    if s.abs() < f64::MIN_POSITIVE || !s.is_finite() {
        *s = 0.0;
    }
}

fn gain_computer(x: f64, t: f64, r: f64, w: f64) -> f64 {
    let d = x - t;
    if 2.0 * d < -w {
        x
    } else if w > 0.0 && 2.0 * d.abs() <= w {
        x + (1.0 / r - 1.0) * (d + w / 2.0).powi(2) / (2.0 * w)
    } else {
        t + d / r
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompParams {
    pub threshold_db: f64,
    pub ratio: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
    pub knee_db: f64,
    pub makeup_db: f64,
    pub auto_makeup: bool,
}

impl Default for CompParams {
    fn default() -> Self {
        Self { threshold_db: -20.0, ratio: 3.0, attack_ms: 5.0, release_ms: 120.0, knee_db: 6.0, makeup_db: 0.0, auto_makeup: true }
    }
}

impl CompParams {
    /// Compensa metade da redução que um sinal em 0 dBFS sofreria.
    pub fn total_makeup(&self) -> f64 {
        let auto = if self.auto_makeup { -gain_computer(0.0, self.threshold_db, self.ratio.max(1.0), self.knee_db.max(0.0)) / 2.0 } else { 0.0 };
        self.makeup_db + auto
    }
}

pub struct Comp {
    p: CompParams,
    aa: f64,
    ar: f64,
    makeup: f64,
    y1: f64,
    yl: f64,
    pub gr_db: f32,
}

impl Comp {
    pub fn new(p: CompParams) -> Comp {
        let mut c = Comp { p, aa: 0.0, ar: 0.0, makeup: 0.0, y1: 0.0, yl: 0.0, gr_db: 0.0 };
        c.set(p);
        c
    }

    pub fn set(&mut self, p: CompParams) {
        self.p = p;
        self.aa = coef(p.attack_ms);
        self.ar = coef(p.release_ms);
        self.makeup = p.total_makeup();
    }

    pub fn process(&mut self, x: &mut [f32]) {
        let (t, r, w) = (self.p.threshold_db, self.p.ratio.max(1.0), self.p.knee_db.max(0.0));
        let mut gr = 0.0f64;
        for v in x.iter_mut() {
            let xg = to_db(*v as f64);
            let xl = xg - gain_computer(xg, t, r, w);
            self.y1 = xl.max(self.ar * self.y1 + (1.0 - self.ar) * xl);
            self.yl = self.aa * self.yl + (1.0 - self.aa) * self.y1;
            gr = gr.max(self.yl);
            *v *= ((self.makeup - self.yl) * DB_TO_LN).exp() as f32;
        }
        flush_f64(&mut self.y1);
        flush_f64(&mut self.yl);
        self.gr_db = gr as f32;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeEssParams {
    pub freq_hz: f64,
    pub threshold_db: f64,
    pub ratio: f64,
    pub range_db: f64,
    /// true: reduz só acima de `freq_hz`; false: reduz o sinal inteiro.
    pub split: bool,
}

impl Default for DeEssParams {
    fn default() -> Self {
        Self { freq_hz: 5500.0, threshold_db: -30.0, ratio: 4.0, range_db: 12.0, split: true }
    }
}

pub struct DeEss {
    p: DeEssParams,
    side: [Biquad; 2],
    /// Crossover Linkwitz-Riley de 4ª ordem: graves + agudos somam plano.
    lo: [Biquad; 2],
    hi: [Biquad; 2],
    aa: f64,
    ar: f64,
    y1: f64,
    yl: f64,
    pub gr_db: f32,
}

fn hp(f: f64) -> Coefs {
    Coefs::new(&BqParams { kind: BqType::HighPass, freq: f, gain: 0.0, width: std::f64::consts::FRAC_1_SQRT_2, width_kind: Width::Q, corner: false }, SR as f64)
}

fn lp(f: f64) -> Coefs {
    Coefs::new(&BqParams { kind: BqType::LowPass, freq: f, gain: 0.0, width: std::f64::consts::FRAC_1_SQRT_2, width_kind: Width::Q, corner: false }, SR as f64)
}

fn peak(f: f64) -> Coefs {
    Coefs::new(&BqParams { kind: BqType::Peak, freq: f * 1.4, gain: 6.0, width: 1.0, width_kind: Width::Q, corner: false }, SR as f64)
}

impl DeEss {
    pub fn new(p: DeEssParams) -> DeEss {
        let f = p.freq_hz.clamp(1000.0, 16000.0);
        let mut d = DeEss {
            p,
            side: [Biquad::new(hp(f)), Biquad::new(peak(f))],
            lo: [Biquad::new(lp(f)), Biquad::new(lp(f))],
            hi: [Biquad::new(hp(f)), Biquad::new(hp(f))],
            aa: coef(1.0), ar: coef(60.0), y1: 0.0, yl: 0.0, gr_db: 0.0 };
        d.set(p);
        d
    }

    pub fn set(&mut self, p: DeEssParams) {
        let f = p.freq_hz.clamp(1000.0, 16000.0);
        self.side[0].c = hp(f);
        self.side[1].c = peak(f);
        for k in 0..2 {
            self.lo[k].c = lp(f);
            self.hi[k].c = hp(f);
        }
        self.p = p;
    }

    pub fn process(&mut self, x: &mut [f32]) {
        let (t, r, range) = (self.p.threshold_db, self.p.ratio.max(1.0), self.p.range_db.max(0.0));
        let mut gr = 0.0f64;
        for v in x.iter_mut() {
            let s = *v as f64;
            let h0 = self.side[0].tick(s);
            let sc = self.side[1].tick(h0);
            let xg = to_db(sc);
            let xl = (xg - gain_computer(xg, t, r, 3.0)).min(range);
            self.y1 = xl.max(self.ar * self.y1 + (1.0 - self.ar) * xl);
            self.yl = self.aa * self.yl + (1.0 - self.aa) * self.y1;
            gr = gr.max(self.yl);
            let g = if self.yl <= 0.0 { 1.0 } else { (-self.yl * DB_TO_LN).exp() };
            *v = if self.p.split {
                let l1 = self.lo[0].tick(s);
                let l = self.lo[1].tick(l1);
                let h1 = self.hi[0].tick(s);
                let h = self.hi[1].tick(h1);
                (l + h * g) as f32
            } else {
                (s * g) as f32
            };
        }
        flush_f64(&mut self.y1);
        flush_f64(&mut self.yl);
        for b in self.side.iter_mut().chain(self.lo.iter_mut()).chain(self.hi.iter_mut()) {
            b.flush_denormals();
        }
        self.gr_db = gr as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f64, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f64::consts::PI * f * i as f64 / SR as f64).sin() as f32).collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn comp_reduz_acima_do_limiar() {
        let p = CompParams { threshold_db: -20.0, ratio: 4.0, knee_db: 0.0, auto_makeup: false, ..Default::default() };
        let mut c = Comp::new(p);
        let mut x = tone(1000.0, 1.0, SR);
        c.process(&mut x);
        // pico 0 dB -> -20 + 20/4 = -15 dB
        let pk = x[SR / 2..].iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!((to_db(pk as f64) + 15.0).abs() < 1.0, "{}", to_db(pk as f64));
        let mut q = tone(1000.0, 0.01, SR);
        let before = rms(&q);
        c.process(&mut q);
        assert!((rms(&q[SR / 2..]) / before - 1.0).abs() < 0.05);
    }

    #[test]
    fn comp_makeup() {
        let p = CompParams { threshold_db: -20.0, ratio: 4.0, knee_db: 0.0, makeup_db: 6.0, auto_makeup: false, ..Default::default() };
        let mut c = Comp::new(p);
        let mut q = tone(1000.0, 0.01, SR);
        let before = rms(&q);
        c.process(&mut q);
        assert!((20.0 * (rms(&q[SR / 2..]) / before).log10() - 6.0).abs() < 0.2);
        assert!((CompParams { threshold_db: -20.0, ratio: 4.0, knee_db: 0.0, ..Default::default() }.total_makeup() - 7.5).abs() < 1e-9);
    }

    #[test]
    fn deesser_so_nos_agudos() {
        let mut d = DeEss::new(DeEssParams::default());
        let mut s = tone(7000.0, 0.5, SR);
        let b = rms(&s[SR / 2..]);
        d.process(&mut s);
        assert!(rms(&s[SR / 2..]) < b * 0.5, "chiado não reduziu");
        let mut d = DeEss::new(DeEssParams::default());
        let mut v = tone(300.0, 0.5, SR);
        let b = rms(&v[SR / 2..]);
        d.process(&mut v);
        assert!((rms(&v[SR / 2..]) / b - 1.0).abs() < 0.05, "voz grave mudou");
    }
}
