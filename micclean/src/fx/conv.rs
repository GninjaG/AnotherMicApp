//! Convolução particionada uniforme (overlap-save), bloco = FRAME. Substitui o libHybridConv do EAPO.
use crate::FRAME;
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::{Arc, OnceLock};

type Plans = (Arc<dyn RealToComplex<f32>>, Arc<dyn ComplexToReal<f32>>);

fn plans() -> &'static Plans {
    static P: OnceLock<Plans> = OnceLock::new();
    P.get_or_init(|| {
        let mut pl = RealFftPlanner::<f32>::new();
        (pl.plan_fft_forward(2 * FRAME), pl.plan_fft_inverse(2 * FRAME))
    })
}

pub struct Conv {
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    parts: Vec<Complex32>,
    fdl: Vec<Complex32>,
    np: usize,
    bins: usize,
    identity: bool,
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
        let (fwd, inv) = plans();
        let (fwd, inv) = (fwd.clone(), inv.clone());
        let bins = n / 2 + 1;
        let ir = if ir.is_empty() {
            &[1.0f32][..]
        } else {
            let end = ir.iter().rposition(|&v| v.abs() >= 1e-6).map(|i| i + 1).unwrap_or(1);
            &ir[..end]
        };
        let identity = ir.len() == 1 && (ir[0] - 1.0).abs() < 1e-6;
        let np = ir.len().div_ceil(FRAME);
        if identity {
            return Conv {
                fwd,
                inv,
                parts: Vec::new(),
                fdl: Vec::new(),
                np: 1,
                bins,
                identity: true,
                pos: 0,
                prev: Vec::new(),
                tbuf: Vec::new(),
                acc: Vec::new(),
                scratch_f: Vec::new(),
                scratch_i: Vec::new(),
            };
        }
        let mut sf = fwd.make_scratch_vec();
        let mut parts = vec![Complex32::default(); np * bins];
        let mut t = vec![0.0f32; n];
        let scale = 1.0 / n as f32;
        for k in 0..np {
            t.fill(0.0);
            let seg = &ir[k * FRAME..((k + 1) * FRAME).min(ir.len())];
            for (d, s) in t.iter_mut().zip(seg) {
                *d = *s * scale;
            }
            let _ = fwd.process_with_scratch(&mut t, &mut parts[k * bins..(k + 1) * bins], &mut sf);
        }
        Conv {
            fdl: vec![Complex32::default(); np * bins],
            parts,
            np,
            bins,
            identity: false,
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
        if self.identity { 1 } else { self.np * FRAME }
    }

    pub fn process(&mut self, x: &mut [f32]) {
        debug_assert_eq!(x.len(), FRAME);
        if self.identity {
            return;
        }
        let np = self.np;
        let bins = self.bins;
        self.tbuf[..FRAME].copy_from_slice(&self.prev);
        self.tbuf[FRAME..].copy_from_slice(x);
        self.prev.copy_from_slice(x);
        let pos_off = self.pos * bins;
        let _ = self.fwd.process_with_scratch(&mut self.tbuf, &mut self.fdl[pos_off..pos_off + bins], &mut self.scratch_f);
        {
            let h0 = &self.parts[..bins];
            let x0 = &self.fdl[pos_off..pos_off + bins];
            for ((a, &hv), &xv) in self.acc.iter_mut().zip(h0).zip(x0) {
                *a = hv * xv;
            }
        }
        for k in 1..np {
            let xi = (self.pos + np - k) % np;
            let h = &self.parts[k * bins..(k + 1) * bins];
            let xs = &self.fdl[xi * bins..(xi + 1) * bins];
            for ((a, &hv), &xv) in self.acc.iter_mut().zip(h).zip(xs) {
                *a += hv * xv;
            }
        }
        let last = bins - 1;
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
    if sp.channels == 0 {
        return Err("arquivo de IR sem canais".into());
    }
    if sp.sample_rate == 0 {
        return Err("taxa de amostragem inválida".into());
    }
    let ch = sp.channels as usize;
    let all: Vec<f32> = match sp.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            if sp.bits_per_sample == 0 || sp.bits_per_sample > 32 {
                return Err(format!("profundidade de bits inválida: {}", sp.bits_per_sample));
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::{chain::*, geq};

    #[test]
    fn trimmed_conv_e_geq_identidade() {
        let mut ir = vec![0.0f32; 4096];
        ir[0] = 0.5;
        ir[120] = -0.25;
        let mut full = Conv::new(&ir);
        assert!(full.taps() < 4096);
        let mut x: Vec<f32> = (0..FRAME).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        let orig = x.clone();
        full.process(&mut x);
        assert!((x[0] - orig[0] * 0.5).abs() < 1e-5);
        assert!((x[120] - (orig[120] * 0.5 - orig[0] * 0.25)).abs() < 1e-5);

        let nodes: Vec<(f64, f64)> = geq::BANDS_15.iter().map(|&f| (f, 0.0)).collect();
        let mut vst = Default::default();
        let mut bld = Builder { vst: &mut vst, vol_db: 0.0, base: std::path::Path::new(".") };
        let (fx, err) = bld.build(&BlockCfg { id: 1, enabled: true, kind: Kind::GraphicEq { mode: GeqMode::Bands15, nodes } });
        assert!(err.is_none());
        match fx {
            Fx::Conv(mut c) => {
                assert!(c.identity);
                let mut z = orig.clone();
                c.process(&mut z);
                assert_eq!(z, orig);
            }
            _ => panic!("esperava Conv"),
        }
    }
}
