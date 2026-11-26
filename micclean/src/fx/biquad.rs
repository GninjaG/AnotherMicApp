// Portado de EqualizerAPO filters/BiQuad.cpp e BiQuadFilter.cpp (GPL-2.0, Jonas Thedering).
use serde::{Deserialize, Serialize};
use std::f64::consts::{LN_2, PI};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BqType {
    Peak,
    LowPass,
    HighPass,
    BandPass,
    LowShelf,
    HighShelf,
    Notch,
    AllPass,
}

impl BqType {
    pub const ALL: [BqType; 8] = [
        BqType::Peak,
        BqType::LowShelf,
        BqType::HighShelf,
        BqType::HighPass,
        BqType::LowPass,
        BqType::BandPass,
        BqType::Notch,
        BqType::AllPass,
    ];
    pub fn label(self) -> &'static str {
        match self {
            BqType::Peak => "Peak",
            BqType::LowPass => "Low-pass",
            BqType::HighPass => "High-pass",
            BqType::BandPass => "Band-pass",
            BqType::LowShelf => "Low shelf",
            BqType::HighShelf => "High shelf",
            BqType::Notch => "Notch",
            BqType::AllPass => "All-pass",
        }
    }
    pub fn has_gain(self) -> bool {
        matches!(self, BqType::Peak | BqType::LowShelf | BqType::HighShelf)
    }
    pub fn is_shelf(self) -> bool {
        matches!(self, BqType::LowShelf | BqType::HighShelf)
    }
}

/// Como interpretar `width`: Q, largura em oitavas ou slope em dB (shelves).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Width {
    Q,
    Bw,
    Slope,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BqParams {
    pub kind: BqType,
    pub freq: f64,
    pub gain: f64,
    pub width: f64,
    pub width_kind: Width,
    /// Shelves sem "C" no EAPO (LS/HS) usam frequência de canto estilo DCX2496.
    pub corner: bool,
}

impl Default for BqParams {
    fn default() -> Self {
        Self { kind: BqType::Peak, freq: 1000.0, gain: 0.0, width: 1.0, width_kind: Width::Q, corner: false }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Coefs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coefs {
    pub fn new(p: &BqParams, srate: f64) -> Coefs {
        let kind = p.kind;
        let mut gain = p.gain;
        if !kind.has_gain() {
            gain = 0.0;
        }
        // S do EAPO é normalizado: slope de 12 dB = 1
        let (bw, is_bw_or_s) = if p.width <= 0.0 {
            match kind {
                BqType::LowShelf | BqType::HighShelf => (0.9, true),
                BqType::Notch => (30.0, false),
                _ => (std::f64::consts::FRAC_1_SQRT_2, false),
            }
        } else {
            match p.width_kind {
                Width::Q => (p.width, false),
                Width::Bw if !kind.is_shelf() => (p.width, true),
                Width::Bw => (p.width, false),
                Width::Slope if kind.is_shelf() => (p.width / 12.0, true),
                Width::Slope => (std::f64::consts::FRAC_1_SQRT_2, false),
            }
        };
        let mut freq = p.freq.clamp(1.0, srate * 0.499);
        if p.corner && kind.is_shelf() {
            let s = if !is_bw_or_s {
                let a = 10f64.powf(gain / 40.0);
                1.0 / ((1.0 / (bw * bw) - 2.0) / (a + 1.0 / a) + 1.0)
            } else {
                bw
            };
            if s > 1e-6 && s.is_finite() {
                let f = 10f64.powf(gain.abs() / 80.0 / s);
                if f.is_finite() {
                    if kind == BqType::LowShelf { freq *= f } else { freq /= f }
                    freq = freq.clamp(1.0, srate * 0.499);
                }
            }
        }
        let a = if matches!(kind, BqType::Peak | BqType::LowShelf | BqType::HighShelf) {
            10f64.powf(gain / 40.0)
        } else {
            10f64.powf(gain / 20.0)
        };
        let w = 2.0 * PI * freq / srate;
        let (sn, cs) = (w.sin(), w.cos());
        let alpha = if !is_bw_or_s {
            sn / (2.0 * bw)
        } else if kind.is_shelf() {
            sn / 2.0 * ((a + 1.0 / a) * (1.0 / bw - 1.0) + 2.0).max(0.0).sqrt()
        } else {
            sn * (LN_2 / 2.0 * bw * w / sn).sinh()
        };
        let beta = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = match kind {
            BqType::LowPass => ((1.0 - cs) / 2.0, 1.0 - cs, (1.0 - cs) / 2.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BqType::HighPass => ((1.0 + cs) / 2.0, -(1.0 + cs), (1.0 + cs) / 2.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BqType::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BqType::Notch => (1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BqType::AllPass => (1.0 - alpha, -2.0 * cs, 1.0 + alpha, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
            BqType::Peak => (1.0 + alpha * a, -2.0 * cs, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cs, 1.0 - alpha / a),
            BqType::LowShelf => (
                a * ((a + 1.0) - (a - 1.0) * cs + beta),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - beta),
                (a + 1.0) + (a - 1.0) * cs + beta,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - beta,
            ),
            BqType::HighShelf => (
                a * ((a + 1.0) + (a - 1.0) * cs + beta),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - beta),
                (a + 1.0) - (a - 1.0) * cs + beta,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - beta,
            ),
        };
        let c = Coefs { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 };
        if [c.b0, c.b1, c.b2, c.a1, c.a2].iter().all(|v| v.is_finite()) { c } else { Coefs { b0: 1.0, ..Default::default() } }
    }

    pub fn gain_at(&self, freq: f64, srate: f64) -> f64 {
        let w = 2.0 * PI * freq / srate;
        let s = (w / 2.0).sin();
        let phi = s * s;
        let (b0, b1, b2, a1, a2) = (self.b0, self.b1, self.b2, self.a1, self.a2);
        let num = (b0 + b1 + b2).powi(2) - 4.0 * (b0 * b1 + 4.0 * b0 * b2 + b1 * b2) * phi + 16.0 * b0 * b2 * phi * phi;
        let den = (1.0 + a1 + a2).powi(2) - 4.0 * (a1 + 4.0 * a2 + a1 * a2) * phi + 16.0 * a2 * phi * phi;
        10.0 * num.max(1e-30).log10() - 10.0 * den.max(1e-30).log10()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    pub c: Coefs,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    pub fn new(c: Coefs) -> Self {
        Self { c, ..Default::default() }
    }

    #[inline]
    pub fn tick(&mut self, x: f64) -> f64 {
        let c = &self.c;
        let y = c.b0 * x + c.b2 * self.x2 + c.b1 * self.x1 - c.a2 * self.y2 - c.a1 * self.y1;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    pub fn flush_denormals(&mut self) {
        for s in [&mut self.x1, &mut self.x2, &mut self.y1, &mut self.y2] {
            if s.abs() < f64::MIN_POSITIVE || !s.is_finite() {
                *s = 0.0;
            }
        }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for v in buf.iter_mut() {
            *v = self.tick(*v as f64) as f32;
        }
        self.flush_denormals();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_extreme_q_nao_gera_nan() {
        for q in [0.001, 0.1, 1.0, 2.0, 10.0, 100.0] {
            for kind in [BqType::LowShelf, BqType::HighShelf] {
                let p = BqParams { kind, freq: 200.0, gain: 12.0, width: q, width_kind: Width::Q, corner: true };
                let c = Coefs::new(&p, 48000.0);
                let g = c.gain_at(1000.0, 48000.0);
                assert!(g.is_finite(), "NaN em Q={q}");
            }
        }
    }
}
