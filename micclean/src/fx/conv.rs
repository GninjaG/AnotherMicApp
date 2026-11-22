//! Convolução particionada uniforme (overlap-save), bloco = FRAME. Substitui o libHybridConv do EAPO.
use crate::FRAME;
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub struct Conv {
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    parts: Vec<Vec<Complex32>>,
    fdl: Vec<Vec<Complex32>>,
    pos: usize,
    prev: Vec<f32>,
    tbuf: Vec<f32>,
    acc: Vec<Complex32>,
    scratch_f: Vec<Complex32>,
    scratch_i: Vec<Complex32>,
}

impl Conv {
    /// Prepara o kernel (aloca; fora da thread de áudio).
    pub fn new(ir: &[f32]) -> Conv {
        let n = 2 * FRAME;
        let mut pl = RealFftPlanner::<f32>::new();
        let fwd = pl.plan_fft_forward(n);
        let inv = pl.plan_fft_inverse(n);
        let bins = n / 2 + 1;
        let ir = if ir.is_empty() { &[1.0f32][..] } else { ir };
        let np = ir.len().div_ceil(FRAME);
        let mut sf = fwd.make_scratch_vec();
        let mut parts = Vec::with_capacity(np);
        let scale = 1.0 / n as f32;
        for k in 0..np {
            let mut t = vec![0.0f32; n];
            let seg = &ir[k * FRAME..((k + 1) * FRAME).min(ir.len())];
            for (d, s) in t.iter_mut().zip(seg) {
                *d = *s * scale;
            }
            let mut h = vec![Complex32::default(); bins];
            let _ = fwd.process_with_scratch(&mut t, &mut h, &mut sf);
            parts.push(h);
        }
        Conv {
            fdl: vec![vec![Complex32::default(); bins]; np],
            parts,
            pos: 0,
            prev: vec![0.0; FRAME],
            tbuf: vec![0.0; n],
            acc: vec![Complex32::default(); bins],
            scratch_f: sf,
            scratch_i: inv.make_scratch_vec(),
            fwd,
            inv,
        }
    }

    pub fn taps(&self) -> usize {
        self.parts.len() * FRAME
    }

    pub fn process(&mut self, x: &mut [f32]) {
        debug_assert_eq!(x.len(), FRAME);
        let np = self.parts.len();
        self.tbuf[..FRAME].copy_from_slice(&self.prev);
        self.tbuf[FRAME..].copy_from_slice(x);
        self.prev.copy_from_slice(x);
        let _ = self.fwd.process_with_scratch(&mut self.tbuf, &mut self.fdl[self.pos], &mut self.scratch_f);
        self.acc.iter_mut().for_each(|c| *c = Complex32::default());
        for k in 0..np {
            let xi = (self.pos + np - k) % np;
            let (h, xs) = (&self.parts[k], &self.fdl[xi]);
            for ((a, hv), xv) in self.acc.iter_mut().zip(h).zip(xs) {
                *a += hv * xv;
            }
        }
        let last = self.acc.len() - 1;
        self.acc[0].im = 0.0;
        self.acc[last].im = 0.0;
        let _ = self.inv.process_with_scratch(&mut self.acc, &mut self.tbuf, &mut self.scratch_i);
        x.copy_from_slice(&self.tbuf[FRAME..]);
        self.pos = (self.pos + 1) % np;
    }
}

/// Lê um .wav como IR mono (1º canal) a 48 kHz, reamostrando se preciso.
pub fn load_ir(path: &std::path::Path) -> Result<Vec<f32>, String> {
    let mut r = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let sp = r.spec();
    let ch = sp.channels.max(1) as usize;
    let all: Vec<f32> = match sp.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let m = (1i64 << (sp.bits_per_sample - 1)) as f32;
            r.samples::<i32>().filter_map(Result::ok).map(|v| v as f32 / m).collect()
        }
    };
    let mono: Vec<f32> = all.chunks(ch).map(|c| c[0]).collect();
    if mono.is_empty() {
        return Err("arquivo de IR vazio".into());
    }
    if sp.sample_rate as usize == crate::SR {
        return Ok(mono);
    }
    Ok(resample(&mono, sp.sample_rate as f64, crate::SR as f64))
}

/// Reamostragem offline com sinc janelado (só para carregar IRs).
fn resample(x: &[f32], from: f64, to: f64) -> Vec<f32> {
    let ratio = to / from;
    let fc = ratio.min(1.0) * 0.95;
    let half = 32.0 / fc;
    let n = (x.len() as f64 * ratio).round() as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / ratio;
            let (lo, hi) = ((t - half).ceil().max(0.0) as usize, ((t + half).floor() as usize).min(x.len() - 1));
            let mut acc = 0.0;
            for (k, &v) in x.iter().enumerate().take(hi + 1).skip(lo) {
                let d = k as f64 - t;
                let s = if d.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * fc * d).sin() / (std::f64::consts::PI * fc * d) };
                let w = 0.5 * (1.0 + (std::f64::consts::PI * d / half).cos());
                acc += v as f64 * fc * s * w;
            }
            acc as f32
        })
        .collect()
}
