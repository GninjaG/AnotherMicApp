#![allow(dead_code)]
use micclean::{FRAME, SR};
use std::path::PathBuf;

pub fn testdata(name: &str) -> Vec<f32> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata").join(name);
    let (x, sr) = micclean::wav::read_mono(&p)
        .unwrap_or_else(|e| panic!("{e} - rode scripts/gen_speech.ps1 antes"));
    assert_eq!(sr as usize, SR);
    x
}

pub fn out_path(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target_test_out");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

pub struct Lcg(pub u64);
impl Lcg {
    pub fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

/// Resposta de sala sintética: atraso + reflexões + cauda + passa-baixa.
pub fn room_ir(delay_ms: f32, gain: f32, seed: u64) -> Vec<f32> {
    let d = (delay_ms * SR as f32 / 1000.0) as usize;
    let len = d + SR * 60 / 1000;
    let mut h = vec![0.0f32; len];
    let mut r = Lcg(seed);
    h[d] = 1.0;
    for (ms, a) in [(2.5, 0.45), (6.0, -0.3), (11.0, 0.2), (17.0, -0.12)] {
        h[d + (ms * SR as f32 / 1000.0) as usize] += a;
    }
    for i in 0..SR * 60 / 1000 {
        let t = i as f32 / SR as f32;
        h[d + i] += 0.05 * r.next() * (-t / 0.015).exp();
    }
    let mut y = 0.0;
    for v in h.iter_mut() {
        y = 0.6 * y + 0.4 * *v;
        *v = y;
    }
    let e: f32 = h.iter().map(|x| x * x).sum::<f32>().sqrt();
    h.iter_mut().for_each(|v| *v *= gain / e);
    h
}

pub fn convolve(x: &[f32], h: &[f32]) -> Vec<f32> {
    let nz: Vec<(usize, f32)> = h.iter().cloned().enumerate().filter(|(_, v)| v.abs() > 1e-6).collect();
    let mut y = vec![0.0f32; x.len()];
    for (k, hk) in nz {
        for n in k..x.len() {
            y[n] += hk * x[n - k];
        }
    }
    y
}

pub fn energy(x: &[f32]) -> f64 {
    x.iter().map(|&v| (v as f64) * (v as f64)).sum()
}

pub fn db(x: f64) -> f64 {
    10.0 * x.max(1e-20).log10()
}

pub fn rms_db(x: &[f32]) -> f64 {
    db(energy(x) / x.len().max(1) as f64)
}

pub fn white(n: usize, level: f32, seed: u64) -> Vec<f32> {
    let mut r = Lcg(seed);
    (0..n).map(|_| r.next() * level).collect()
}

pub fn fan_noise(n: usize, seed: u64) -> Vec<f32> {
    let mut r = Lcg(seed);
    let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
    (0..n)
        .map(|i| {
            let w = r.next();
            b0 = 0.99765 * b0 + w * 0.0990460;
            b1 = 0.96300 * b1 + w * 0.2965164;
            b2 = 0.57000 * b2 + w * 1.0526913;
            let pink = (b0 + b1 + b2 + w * 0.1848) * 0.2;
            let t = i as f32 / SR as f32;
            let hum = 0.15 * (2.0 * std::f32::consts::PI * 120.0 * t).sin()
                + 0.08 * (2.0 * std::f32::consts::PI * 240.0 * t).sin()
                + 0.05 * (2.0 * std::f32::consts::PI * 1530.0 * t).sin();
            pink * (1.0 + 0.1 * (2.0 * std::f32::consts::PI * 0.7 * t).sin()) + hum
        })
        .collect()
}

pub fn keyboard_noise(n: usize, seed: u64) -> Vec<f32> {
    let mut r = Lcg(seed);
    let mut y = vec![0.0f32; n];
    let mut t = 0usize;
    while t < n {
        t += (SR as f32 * (0.08 + 0.17 * (r.next() * 0.5 + 0.5))) as usize;
        let amp = 0.6 + 0.4 * r.next().abs();
        let len = SR * 25 / 1000;
        let mut lp = 0.0;
        for i in 0..len {
            if t + i >= n {
                break;
            }
            let env = (-(i as f32) / (SR as f32 * 0.004)).exp();
            lp = 0.5 * lp + 0.5 * r.next();
            y[t + i] += amp * env * (r.next() - lp);
        }
    }
    y
}

pub fn babble(n: usize) -> Vec<f32> {
    let a = testdata("babble1.wav");
    let b = testdata("babble2.wav");
    let c = testdata("near_en.wav");
    (0..n)
        .map(|i| a[i % a.len()] + b[(i + 7919) % b.len()] + 0.7 * c[(i + 104729) % c.len()])
        .collect()
}

pub fn fit(x: &[f32], n: usize) -> Vec<f32> {
    (0..n).map(|i| x[i % x.len()]).collect()
}

pub fn scale_to_snr(clean: &[f32], noise: &[f32], snr_db: f64) -> Vec<f32> {
    let g = (energy(clean) / energy(noise) / 10f64.powf(snr_db / 10.0)).sqrt() as f32;
    noise.iter().map(|v| v * g).collect()
}

/// Atraso (em amostras) de y em relação a x, procurando 0..max.
pub fn find_delay(x: &[f32], y: &[f32], max: usize) -> usize {
    let n = x.len().min(y.len()).saturating_sub(max);
    let mut best = (0, f64::MIN);
    for d in 0..=max {
        let c: f64 = (0..n).step_by(2).map(|i| x[i] as f64 * y[i + d] as f64).sum();
        if c > best.1 {
            best = (d, c);
        }
    }
    best.0
}

pub fn si_sdr(reference: &[f32], est: &[f32]) -> f64 {
    let n = reference.len().min(est.len());
    let (r, e) = (&reference[..n], &est[..n]);
    let dot: f64 = r.iter().zip(e).map(|(a, b)| *a as f64 * *b as f64).sum();
    let a = dot / energy(r).max(1e-20);
    let num: f64 = r.iter().map(|v| (a * *v as f64).powi(2)).sum();
    let den: f64 = r.iter().zip(e).map(|(rv, ev)| (a * *rv as f64 - *ev as f64).powi(2)).sum();
    db(num / den.max(1e-20))
}

pub fn pearson(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    let ma = a[..n].iter().map(|&v| v as f64).sum::<f64>() / n as f64;
    let mb = b[..n].iter().map(|&v| v as f64).sum::<f64>() / n as f64;
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (x, y) = (a[i] as f64 - ma, b[i] as f64 - mb);
        sab += x * y;
        saa += x * x;
        sbb += y * y;
    }
    sab / (saa * sbb).sqrt().max(1e-20)
}

/// Roda o AEC frame a frame.
pub fn run_aec(mic: &[f32], reference: &[f32]) -> Vec<f32> {
    let mut aec = micclean::dsp::Aec::new();
    let mut out = vec![0.0f32; mic.len()];
    let n = mic.len() / FRAME * FRAME;
    for i in (0..n).step_by(FRAME) {
        let mut f = [0.0f32; FRAME];
        f.copy_from_slice(&mic[i..i + FRAME]);
        aec.process(&mut f, &reference[i..i + FRAME]);
        out[i..i + FRAME].copy_from_slice(&f);
    }
    out
}

pub fn run_dfn(x: &[f32]) -> Vec<f32> {
    let mut d = micclean::dsp::Dfn::new(100.0, 0.0).unwrap();
    let mut out = vec![0.0f32; x.len()];
    let n = x.len() / FRAME * FRAME;
    for i in (0..n).step_by(FRAME) {
        let mut f = [0.0f32; FRAME];
        f.copy_from_slice(&x[i..i + FRAME]);
        d.process(&mut f);
        out[i..i + FRAME].copy_from_slice(&f);
    }
    out
}

/// ERLE em janelas de `win` amostras onde o eco está ativo.
pub fn windowed_erle(mic: &[f32], out: &[f32], echo: &[f32], win: usize) -> Vec<(f64, Option<f64>)> {
    let mut v = Vec::new();
    for s in (0..mic.len().min(out.len()) - win).step_by(win) {
        let t = s as f64 / SR as f64;
        if rms_db(&echo[s..s + win]) < -45.0 {
            v.push((t, None));
        } else {
            v.push((t, Some(db(energy(&mic[s..s + win]) / energy(&out[s..s + win]).max(1e-20)))));
        }
    }
    v
}
