use micclean::dsp::{Aec, DcBlock, Dfn};
use micclean::{FRAME, SR};

fn rms_db(x: &[f32]) -> f64 {
    10.0 * (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64 + 1e-20).log10()
}

fn speech_drop(inp: &[f32], out: &[f32], d: usize) -> (f64, f64) {
    let w = SR / 50;
    let n = (inp.len() - d) / w;
    let li: Vec<f64> = (0..n).map(|k| rms_db(&inp[k * w..(k + 1) * w])).collect();
    let lo: Vec<f64> = (0..n).map(|k| rms_db(&out[d + k * w..d + (k + 1) * w])).collect();
    let mut s = li.clone();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let fl = s[n / 10];
    let mut dd: Vec<f64> = (0..n).filter(|&k| li[k] > fl + 20.0).map(|k| li[k] - lo[k]).collect();
    dd.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = dd[dd.len() / 2];
    (med, dd.iter().filter(|&&x| x > med + 15.0).count() as f64 * 100.0 / dd.len() as f64)
}

fn best_delay(a: &[f32], b: &[f32], max: usize) -> usize {
    let n = a.len() - max;
    (0..max)
        .max_by(|&i, &j| {
            let c = |l: usize| (0..n).step_by(2).map(|k| a[k] as f64 * b[k + l] as f64).sum::<f64>();
            c(i).partial_cmp(&c(j)).unwrap()
        })
        .unwrap()
}

#[test]
#[ignore]
fn diag_talk() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dist/pc_tests/talk_mic.wav");
    let (mut x, _) = micclean::wav::read_mono(&p).unwrap();
    x.truncate(10 * SR / FRAME * FRAME);
    DcBlock::default().process(&mut x);
    println!("\nmic 0-10 s: {:.1} dBFS, pico {:.3}", rms_db(&x), x.iter().fold(0f32, |a, v| a.max(v.abs())));
    let zero = [0.0f32; FRAME];
    let bands = |y: &[f32]| -> String {
        let n = 1 << 18;
        let mut re: Vec<f64> = y[..n].iter().map(|&v| v as f64).collect();
        let e = |lo: f64, hi: f64, re: &Vec<f64>| -> f64 {
            // energia por banda via Goertzel grosso: soma de senoides de teste
            let mut t = 0.0;
            let mut f = lo;
            while f < hi {
                let w = 2.0 * std::f64::consts::PI * f / SR as f64;
                let (mut c, mut s) = (0.0, 0.0);
                for (i, v) in re.iter().enumerate().step_by(1) {
                    c += v * (w * i as f64).cos();
                    s += v * (w * i as f64).sin();
                }
                t += c * c + s * s;
                f += (hi - lo) / 6.0;
            }
            10.0 * (t + 1e-20).log10()
        };
        re.truncate(n);
        [(100.0, 1000.0), (1000.0, 4000.0), (4000.0, 8000.0), (8000.0, 16000.0), (16000.0, 23000.0)]
            .iter()
            .map(|&(a, b)| format!("{:.0}-{:.0}k:{:.0}", a / 1000.0, b / 1000.0, e(a, b, &re)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    println!("  espectro mic: {}", bands(&x));
    let lp = |v: &mut [f32]| {
        // 4 biquads passa-baixas Butterworth em 15 kHz
        let w = 2.0 * std::f64::consts::PI * 15000.0 / SR as f64;
        let (c, sn) = (w.cos(), w.sin());
        let al = sn / (2.0 * 0.7071);
        let (b0, b1, b2, a0, a1, a2) = ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + al, -2.0 * c, 1.0 - al);
        for _ in 0..4 {
            let (mut x1, mut x2, mut y1, mut y2) = (0.0f64, 0.0, 0.0, 0.0);
            for s in v.iter_mut() {
                let x0 = *s as f64;
                let y0 = (b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2) / a0;
                x2 = x1; x1 = x0; y2 = y1; y1 = y0;
                *s = y0 as f32;
            }
        }
    };
    {
        let mut y = x.clone();
        lp(&mut y);
        let mut d = Dfn::new(100.0, 0.0).unwrap();
        let z0 = y.clone();
        for f in y.chunks_mut(FRAME) { d.process(f); }
        let dl = best_delay(&z0, &y, 4000);
        let (med, cut) = speech_drop(&z0, &y, dl);
        println!("  [atraso {dl}] passa-baixas 15k + DFN: queda mediana {med:.1} dB | cortados {cut:.1}% | saída {:.1} dBFS", rms_db(&y));
    }
    {
        let mut a = Aec::new();
        let mut ya = x.clone();
        for f in ya.chunks_mut(FRAME) { a.process(f, &zero); }
        for lvl in [1e-4f32, 3e-5, 1e-5] {
            let mut yn = ya.clone();
            let mut r = 12345u32;
            for v in yn.iter_mut() {
                r = r.wrapping_mul(1664525).wrapping_add(1013904223);
                *v += lvl * ((r >> 8) as f32 / 8388608.0 - 1.0);
            }
            let z = yn.clone();
            let mut d = Dfn::new(100.0, 0.0).unwrap();
            for f in yn.chunks_mut(FRAME) { d.process(f); }
            let dl = best_delay(&z, &yn, 4000);
            let (med, cut) = speech_drop(&z, &yn, dl);
            println!("  AEC + ruído branco {:.0} dBFS + DFN: queda {med:.1} dB | cortados {cut:.1}%", 20.0 * (lvl as f64 / 3f64.sqrt()).log10());
        }
        for (nm, mk) in [
            ("AEC sem HPF", (|| Aec::with_config(false, Some(micclean::dsp::ec3_balanced()))) as fn() -> Aec),
            ("AEC webrtc", || Aec::with_config(true, None)),
        ] {
            let mut a = mk();
            let mut y = x.clone();
            for f in y.chunks_mut(FRAME) { a.process(f, &zero); }
            let z = y.clone();
            let mut d = Dfn::new(100.0, 0.0).unwrap();
            for f in y.chunks_mut(FRAME) { d.process(f); }
            let (m1, c1) = speech_drop(&x, &z, best_delay(&x, &z, 4000));
            let (m2, c2) = speech_drop(&z, &y, best_delay(&z, &y, 4000));
            let (m3, c3) = speech_drop(&x, &y, best_delay(&x, &y, 4000));
            println!("  {nm}: AEC {m1:.1} dB/{c1:.0}% | DFN sobre ele {m2:.1} dB/{c2:.0}% | total {m3:.1} dB/{c3:.0}%");
        }
        let first_nz = ya.iter().position(|v| v.abs() > 1e-6).unwrap_or(0);
        println!("  AEC: primeira amostra não nula em {first_nz} ({:.0} ms)", first_nz as f64 * 1000.0 / SR as f64);
        for (nm, k) in [("DFN em x com 0,5 s iniciais zerados", SR / 2), ("DFN em x com 2 s zerados", 2 * SR)] {
            let mut y = x.clone();
            y[..k].fill(0.0);
            let z = y.clone();
            let mut d = Dfn::new(100.0, 0.0).unwrap();
            for f in y.chunks_mut(FRAME) { d.process(f); }
            let (m, c) = speech_drop(&z, &y, best_delay(&z, &y, 4000));
            println!("  {nm}: queda {m:.1} dB | cortados {c:.0}%");
        }
        let mut yd = ya.clone();
        let mut d = Dfn::new(100.0, 0.0).unwrap();
        for f in yd.chunks_mut(FRAME) { d.process(f); }
        let mut xd = x.clone();
        let mut d0 = Dfn::new(100.0, 0.0).unwrap();
        for f in xd.chunks_mut(FRAME) { d0.process(f); }
        let o = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target_test_out");
        let _ = std::fs::create_dir_all(&o);
        for lim in [30.0f32, 20.0, 12.0] {
            let mut a = Aec::new();
            let mut d = Dfn::new(lim, 0.0).unwrap();
            let mut y = x.clone();
            for f in y.chunks_mut(FRAME) { a.process(f, &zero); d.process(f); }
            micclean::wav::write_mono(&o.join(format!("diag_aec_dfn{lim:.0}.wav")), &y, SR as u32).unwrap();
        }
        for (n, v) in [("diag_x.wav", &x), ("diag_aec.wav", &ya), ("diag_aec_dfn.wav", &yd), ("diag_dfn.wav", &xd)] {
            micclean::wav::write_mono(&o.join(n), v, SR as u32).unwrap();
        }
        let dl = best_delay(&ya, &yd, 4000);
        let (med, cut) = speech_drop(&ya, &yd, dl);
        println!("  [atraso {dl}] DFN medido contra a SAÍDA do AEC: queda {med:.1} dB | cortados {cut:.1}%");
        let mut d2 = Dfn::new(100.0, 0.0).unwrap();
        let mut a2 = Aec::new();
        let mut yr = x.clone();
        for f in yr.chunks_mut(FRAME) { d2.process(f); a2.process(f, &zero); }
        let dl = best_delay(&x, &yr, 4000);
        let (med, cut) = speech_drop(&x, &yr, dl);
        println!("  [atraso {dl}] ordem invertida DFN -> AEC: queda {med:.1} dB | cortados {cut:.1}% | saída {:.1} dBFS", rms_db(&yr));
    }
    let run = |aec: bool, dfn: Option<f32>| -> Vec<f32> {
        let mut a = aec.then(Aec::new);
        let mut d = dfn.map(|l| Dfn::new(l, 0.0).unwrap());
        let mut y = x.clone();
        for f in y.chunks_mut(FRAME) {
            if let Some(a) = a.as_mut() {
                a.process(f, &zero);
            }
            if let Some(d) = d.as_mut() {
                d.process(f);
            }
        }
        y
    };
    for (name, aec, dfn, d) in [
        ("só AEC (ref silenciosa)", true, None, 431),
        ("só DFN lim 100", false, Some(100.0), 1440),
        ("só DFN lim 30", false, Some(30.0), 1440),
        ("só DFN lim 12", false, Some(12.0), 1440),
        ("AEC + DFN 100", true, Some(100.0), 1871),
    ] {
        let y = run(aec, dfn);
        let _ = d;
        let d = best_delay(&x, &y, 4000);
        let (med, cut) = speech_drop(&x, &y, d);
        print!("  [atraso {d}] {} ", bands(&y));
        println!("  {name:<24} queda mediana na voz {med:5.1} dB | quadros cortados {cut:4.1}% | saída {:.1} dBFS", rms_db(&y));
    }
}
