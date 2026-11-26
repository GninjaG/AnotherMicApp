mod common;
use common::*;
use micclean::config::Config;
use micclean::drift::{OutPath, OutReader, RefPath};
use micclean::dsp::Pipeline;
use micclean::{FRAME, SR};
use rtrb::RingBuffer;

/// Cadeia completa como no app: ref -> RefPath, mic -> AEC -> DFN -> ganho/limiter -> OutPath -> ring -> leitor.
fn run_chain(cfg: &Config, mic: &[f32], reference: &[f32]) -> (Vec<f32>, u64) {
    let mut pipe = Pipeline::from_config(cfg).unwrap();
    let (mut rp, mut rc) = RingBuffer::<f32>::new(SR / 2);
    let (mut op, mut oc) = RingBuffer::<f32>::new(SR / 2);
    let mut refp = RefPath::new(cfg.buffers.ref_ms);
    let mut outp = OutPath::new(cfg.buffers.out_ms, SR / 2);
    let mut reader = OutReader::new(outp.target);
    let n = mic.len() / FRAME * FRAME;
    let mut out = Vec::with_capacity(n);
    let mut f = [0.0f32; FRAME];
    let mut rb = [0.0f32; FRAME];
    let mut ob = [0.0f32; FRAME];
    let mut nan = 0;
    for i in (0..n).step_by(FRAME) {
        for &s in &reference[i..i + FRAME] {
            let _ = rp.push(s);
        }
        refp.pull(&mut rc, &mut rb);
        f.copy_from_slice(&mic[i..i + FRAME]);
        let t = pipe.process(&mut f, &rb);
        nan += t.nan as u64;
        outp.push(&f, &mut op);
        reader.read(&mut oc, &mut ob);
        out.extend_from_slice(&ob);
    }
    (out, nan)
}

fn onset(x: &[f32], thr_db: f64) -> usize {
    let w = SR / 1000;
    (0..x.len() / w).find(|&k| rms_db(&x[k * w..(k + 1) * w]) > thr_db).map(|k| k * w).unwrap_or(usize::MAX)
}

#[test]
fn t6_cadeia_completa() {
    let mut cfg = Config::default();
    cfg.output.gain_db = 6.0;
    let reference = testdata("far_pt.wav");
    let near0 = testdata("near_en.wav");
    let n = reference.len();
    let start = 12 * SR;
    let mut near = vec![0.0f32; n];
    let len = near0.len().min(n - start);
    // segundo trecho da voz local bem alto (+12 dB) para forçar o limiter
    for i in 0..len {
        let g = if i > len / 2 { 4.0 } else { 1.0 };
        near[start + i] = near0[i] * g;
    }
    let echo = convolve(&reference, &room_ir(70.0, 0.17, 3));
    let fan = scale_to_snr(&near[start..start + len], &fan_noise(n, 9)[start..start + len], 10.0);
    let fan_full: Vec<f32> = (0..n).map(|i| fan[i % fan.len()]).collect();
    let mic: Vec<f32> = (0..n).map(|i| echo[i] + near[i] + fan_full[i]).collect();
    let mic_peak = mic.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    let (out, nan) = run_chain(&cfg, &mic, &reference);
    micclean::wav::write_mono(&out_path("t6_mic.wav"), &mic, SR as u32).unwrap();
    micclean::wav::write_mono(&out_path("t6_out.wav"), &out, SR as u32).unwrap();
    let non_finite = out.iter().filter(|v| !v.is_finite()).count();
    let peak = out.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    let clipped = out.iter().filter(|v| v.abs() >= 0.999).count();
    let mut c0 = Config::default();
    c0.output.gain_db = 0.0;
    let (eo, _) = run_chain(&c0, &echo, &reference);
    let echo_only = db(energy(&echo[10 * SR..]) / energy(&eo[10 * SR..]).max(1e-20));

    // Latência: rajada de voz isolada atravessando a cadeia (sem referência)
    let burst = &near0[SR..SR + SR / 2];
    let mut probe = vec![0.0f32; 4 * SR];
    probe[2 * SR..2 * SR + burst.len()].copy_from_slice(burst);
    let (po, _) = run_chain(&cfg, &probe, &vec![0.0; probe.len()]);
    let lat = onset(&po, -50.0) as f64 - onset(&probe, -50.0) as f64;
    let d_dfn = micclean::dsp::Dfn::new(100.0, 0.0).unwrap().delay();
    let outp = OutPath::new(cfg.buffers.out_ms, SR / 2);

    println!("\n[T6] Cadeia completa AEC -> DFN -> ganho +6 dB -> limiter -> resampler -> saída ({:.1} s)", n as f64 / SR as f64);
    println!("  amostras não finitas: {non_finite} | frames com NaN tratados: {nan}");
    println!("  pico entrada {:.3} ({:+.1} dBFS) | pico saída {:.3} ({:+.1} dBFS) | amostras >= 0,999: {clipped}", mic_peak, 20.0 * (mic_peak as f64).log10(), peak, 20.0 * (peak as f64).log10());
    println!("  redução de eco (só eco, ganho 0 dB, t>=10 s, cadeia inteira): {echo_only:.1} dB");
    println!(
        "  latência medida (início da voz, entrada -> saída): {:.1} ms | teórica: AEC ~9 + DFN {:.0} + buffer saída {:.0} + resampler {:.1} ms",
        lat * 1000.0 / SR as f64,
        d_dfn as f64 * 1000.0 / SR as f64,
        cfg.buffers.out_ms,
        (outp.latency_frames() as f64 - outp.target as f64) * 1000.0 / SR as f64
    );
    assert_eq!(non_finite, 0);
    assert_eq!(nan, 0);
    assert!(peak < 0.95, "pico {peak}");
    assert_eq!(clipped, 0);
    assert!(echo_only > 20.0);
    assert!(lat > 0.0 && lat < 0.1 * SR as f64);
}

#[test]
fn t6b_entrada_nan_e_extremos() {
    let cfg = Config::default();
    let sp = testdata("clean_pt.wav");
    let n = 8 * SR;
    let mut mic: Vec<f32> = sp[..n].iter().map(|v| v * 8.0).collect();
    for i in (SR..SR + 480).step_by(7) {
        mic[i] = f32::NAN;
    }
    let mut reference = sp[SR..SR + n].to_vec();
    reference[3 * SR] = f32::INFINITY;
    let (out, nan) = run_chain(&cfg, &mic, &reference);
    let bad = out.iter().filter(|v| !v.is_finite()).count();
    let peak = out.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    let tail = rms_db(&out[6 * SR..]);
    let mut pipe = Pipeline::from_config(&cfg).unwrap();
    let mut f = [f32::NAN; FRAME];
    pipe.process(&mut f, &[0.0; FRAME]);
    println!("
[T6b] Entrada +18 dB com NaN no mic e Inf na referência: saída não finita={bad}, frames com NaN tratados={nan}, pico={peak:.3}, nível no fim={tail:.1} dBFS (recuperou), resets={}", pipe.resets);
    assert_eq!(bad, 0);
    assert!(peak < 0.95);
    assert!(tail > -40.0, "não recuperou após NaN");
}

#[test]
fn t6c_troca_atenuacao_ao_vivo() {
    let cfg = Config::default();
    let mut pipe = Pipeline::from_config(&cfg).unwrap();
    let n = 12 * SR;
    let fan = fan_noise(n, 5);
    let denom = rms_db(&fan).max(-200.0).abs().max(1.0) as f32;
    let fan: Vec<f32> = fan.iter().map(|v| v * 0.03 / denom).collect();
    let mut out = Vec::with_capacity(n);
    let mut f = [0.0f32; FRAME];
    for (k, c) in fan.chunks(FRAME).enumerate() {
        if k == 6 * SR / FRAME {
            pipe.set_atten(12.0);
        }
        f.copy_from_slice(c);
        pipe.process(&mut f, &[0.0; FRAME]);
        out.extend_from_slice(&f);
    }
    let bad = out.iter().filter(|v| !v.is_finite()).count();
    let (a, b) = (rms_db(&out[3 * SR..6 * SR]), rms_db(&out[9 * SR..12 * SR]));
    let inp = rms_db(&fan[9 * SR..]);
    println!("\n[T6c] troca 100 -> 12 dB aos 6 s: ruído antes {a:.1} dBFS, depois {b:.1} dBFS (entrada {inp:.1}, esperado ~{:.1})", inp - 12.0);
    assert_eq!(bad, 0);
    assert!(b > a + 20.0, "limite não aplicou");
    assert!((b - (inp - 12.0)).abs() < 4.0, "atenuação depois da troca fora de 12 dB");
}
