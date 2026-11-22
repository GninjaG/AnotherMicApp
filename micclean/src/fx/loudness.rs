// Portado de EqualizerAPO filters/loudnessCorrection (GPL-2.0, Alexander Walch / Jonas Thedering).
use super::biquad::{BqParams, BqType, Coefs, Width};

#[derive(Debug, Clone, Copy, Default)]
pub struct LoudCoefs {
    pub ls: Coefs,
    pub hs: Coefs,
    pub att: f64,
    pub neutral: bool,
}

/// `vol_db`: volume mestre da reprodução padrão do Windows (dB), como o EAPO usa.
pub fn coefs(ref_level: f64, ref_offset: f64, attenuation: f64, vol_db: f64, sr: f64) -> LoudCoefs {
    let att = attenuation.clamp(0.0, 1.0);
    let diff = ref_level - ref_offset - vol_db;
    let (gls, pre) = if diff > 0.0 {
        let g = diff * 0.55 / (1.0 - 0.55) * att;
        (g, -g)
    } else if diff < 0.0 {
        (diff * 0.55 * (diff / 90.0).exp() * att, 0.0)
    } else {
        (0.0, 0.0)
    };
    let diff2 = ref_level - ref_offset - (vol_db + pre);
    let ghs = if diff2 > 0.0 {
        diff2 * 0.225 * (-diff2 / 100.0).exp() * att
    } else if diff2 < 0.0 {
        diff2 * 0.175 * (diff2 / 80.0).exp() * att
    } else {
        0.0
    };
    // S do EAPO (0,52 e 0,9) -> Width::Slope usa dB/12
    let shelf = |kind, freq, s: f64, gain| {
        Coefs::new(&BqParams { kind, freq, gain, width: s * 12.0, width_kind: Width::Slope, corner: false }, sr)
    };
    LoudCoefs {
        ls: shelf(BqType::LowShelf, 75.0, 0.52, gls),
        hs: shelf(BqType::HighShelf, 10000.0, 0.9, ghs),
        att: (pre / 6.0 * std::f64::consts::LN_2).exp(),
        neutral: gls.abs().max(ghs.abs()) < 0.2,
    }
}
