// Portado de EqualizerAPO filters/GraphicEQFilter.cpp e helpers/GainIterator.cpp (GPL-2.0, Jonas Thedering).
use rustfft::FftPlanner;
use rustfft::num_complex::Complex64;
use std::f64::consts::PI;

pub const BANDS_15: [f64; 15] =
    [25.0, 40.0, 63.0, 100.0, 160.0, 250.0, 400.0, 630.0, 1000.0, 1600.0, 2500.0, 4000.0, 6300.0, 10000.0, 16000.0];
pub const BANDS_31: [f64; 31] = [
    20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0, 500.0, 630.0, 800.0,
    1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0, 8000.0, 10000.0, 12500.0, 16000.0, 20000.0,
];
pub const FILTER_LEN: usize = 16384;

/// Ganho interpolado linearmente em log(f) entre os nós (ordenados por frequência).
pub fn gain_at(nodes: &[(f64, f64)], f: f64) -> f64 {
    if nodes.is_empty() {
        return 0.0;
    }
    let i = nodes.partition_point(|n| n.0 < f);
    if i == 0 {
        return nodes[0].1;
    }
    if i == nodes.len() {
        return nodes[i - 1].1;
    }
    let (l, r) = (nodes[i - 1], nodes[i]);
    if l.1 == r.1 {
        return l.1;
    }
    let t = (f.ln() - l.0.ln()) / (r.0.ln() - l.0.ln());
    l.1 + t * (r.1 - l.1)
}

pub fn sorted(nodes: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> = nodes.iter().copied().filter(|n| n.0 > 0.0 && n.0.is_finite()).collect();
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    v
}

/// FIR de fase mínima (FILTER_LEN taps) como no EAPO.
pub fn design(nodes: &[(f64, f64)], srate: f64) -> Vec<f32> {
    let nodes = sorted(nodes);
    let l = FILTER_LEN;
    let n = 2 * l;
    let mut pl = FftPlanner::<f64>::new();
    let fwd = pl.plan_fft_forward(n);
    let inv = pl.plan_fft_inverse(n);
    let mut fd = vec![Complex64::default(); n];
    for i in 0..l {
        let f = i as f64 * srate / n as f64;
        let g = 10f64.powf(gain_at(&nodes, f) / 20.0);
        fd[i] = Complex64::new(g, 0.0);
        fd[n - i - 1] = Complex64::new(g, 0.0);
    }
    // espectro de fase mínima via cepstro
    let th = 10f64.powf(-100.0 / 20.0);
    for c in fd.iter_mut() {
        *c = Complex64::new(if c.re < th { th.ln() } else { c.re.ln() }, 0.0);
    }
    inv.process(&mut fd);
    let mut td: Vec<Complex64> = fd.iter().map(|c| c / n as f64).collect();
    for i in 1..l {
        let m = td[n - i];
        td[i].re += m.re;
        td[i].im -= m.im;
        td[n - i] = Complex64::default();
    }
    td[l].im *= -1.0;
    fwd.process(&mut td);
    let mut fd: Vec<Complex64> = td.iter().map(|c| Complex64::from_polar(c.re.exp(), c.im)).collect();
    inv.process(&mut fd);
    (0..l)
        .map(|i| {
            let w = 0.5 * (1.0 + (2.0 * PI * i as f64 / (2 * l) as f64).cos());
            (fd[i].re / n as f64 * w) as f32
        })
        .collect()
}
