mod common;
use common::Lcg;
use micclean::drift::{OutPath, OutReader, RefPath};
use micclean::{FRAME, SR};
use rtrb::RingBuffer;

struct SimResult {
    out_under: u64,
    out_over: u64,
    ref_under: u64,
    ref_over: u64,
    out_lat: (f64, f64, f64, f64),
    ref_lat: (f64, f64, f64, f64),
    out_ppm: f64,
    ref_ppm: f64,
    out_start_end: (f64, f64),
}

fn stats(v: &[f64]) -> (f64, f64, f64, f64) {
    let m = v.iter().sum::<f64>() / v.len() as f64;
    let sd = (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
    (m, sd, v.iter().cloned().fold(f64::MAX, f64::min), v.iter().cloned().fold(f64::MIN, f64::max))
}

/// Simula 3 relógios: mic (mestre), caixas (+ref_ppm) e CABLE (+out_ppm), com jitter de ±2 ms.
fn simulate(minutes: f64, out_ppm: f64, ref_ppm: f64, seed: u64) -> SimResult {
    let ring = SR / 2;
    let (mut oprod, mut ocons) = RingBuffer::<f32>::new(ring);
    let (mut rprod, mut rcons) = RingBuffer::<f32>::new(ring);
    let b = micclean::config::Buffers::default();
    let mut outp = OutPath::new(b.out_ms, ring);
    let mut refp = RefPath::new(b.ref_ms);
    let mut reader = OutReader::new(outp.target);
    let mut rng = Lcg(seed);
    let period = FRAME as f64 / SR as f64;
    let (pm, po, pr) = (period, period / (1.0 + out_ppm * 1e-6), period / (1.0 + ref_ppm * 1e-6));
    let (mut tm, mut to, mut tr) = (0.0, 0.003, 0.006);
    let end = minutes * 60.0;
    let warm = 60.0;
    let mut frame = [0.0f32; FRAME];
    let mut refb = [0.0f32; FRAME];
    let mut obuf = vec![0.0f32; FRAME];
    let (mut ou, mut oo, mut ru, mut ro) = (0, 0, 0, 0);
    let (mut olat, mut rlat) = (Vec::new(), Vec::new());
    let (mut olat_start, mut olat_end) = (Vec::new(), Vec::new());
    let mut phase = 0.0f64;
    let mut ph_ref = 0.0f64;
    let jit = |r: &mut Lcg| 0.002 * r.next() as f64;
    let mut t_mic_j = tm + jit(&mut rng);
    let mut t_out_j = to + jit(&mut rng);
    let mut t_ref_j = tr + jit(&mut rng);
    while tm < end {
        let next = t_mic_j.min(t_out_j).min(t_ref_j);
        if next == t_ref_j {
            // caixas: loopback produz FRAME amostras (seno 440 Hz)
            let mut buf = [0.0f32; FRAME];
            for s in buf.iter_mut() {
                *s = (ph_ref * 2.0 * std::f64::consts::PI).sin() as f32 * 0.5;
                ph_ref += 440.0 / SR as f64;
            }
            let n = FRAME.min(rprod.slots());
            if n < FRAME && tr > warm {
                ro += 1;
            }
            for &s in &buf[..n] {
                let _ = rprod.push(s);
            }
            tr += pr;
            t_ref_j = tr + jit(&mut rng);
        } else if next == t_mic_j {
            for s in frame.iter_mut() {
                *s = (phase * 2.0 * std::f64::consts::PI).sin() as f32 * 0.5;
                phase += 1000.0 / SR as f64;
            }
            let e = refp.pull(&mut rcons, &mut refb);
            if tm > warm {
                ru += e.underrun as u64;
                ro += e.overrun as u64;
                rlat.push(refp.ctl.smoothed_fill() * 1000.0 / SR as f64);
            }
            let e = outp.push(&frame, &mut oprod);
            if tm > warm {
                oo += e.overrun as u64;
            }
            tm += pm;
            t_mic_j = tm + jit(&mut rng);
        } else {
            let fill = ocons.slots();
            if reader.read(&mut ocons, &mut obuf) && to > warm {
                ou += 1;
            }
            if to > warm {
                olat.push(fill as f64 * 1000.0 / SR as f64);
                if to > end - 300.0 { olat_end.push(fill as f64 * 1000.0 / SR as f64); }
                if to < warm + 300.0 { olat_start.push(fill as f64 * 1000.0 / SR as f64); }
            }
            to += po;
            t_out_j = to + jit(&mut rng);
        }
    }
    SimResult {
        out_under: ou,
        out_over: oo,
        ref_under: ru,
        ref_over: ro,
        out_lat: stats(&olat),
        ref_lat: stats(&rlat),
        out_ppm: outp.ctl.drift_ppm(),
        ref_ppm: refp.ctl.drift_ppm(),
        out_start_end: (stats(&olat_start).0, stats(&olat_end).0),
    }
}

#[test]
fn t5_resampler_deriva_30min() {
    println!("\n[T5] Deriva de relógio, 30 min simulados, jitter ±2 ms (estatísticas após 60 s)");
    let mut ok = true;
    for (o, r) in [(200.0, -200.0), (-200.0, 200.0), (200.0, 200.0), (0.0, 0.0)] {
        let t = std::time::Instant::now();
        let s = simulate(30.0, o, r, 42);
        println!(
            "  CABLE {o:+.0} ppm / caixas {r:+.0} ppm: underrun out={} ref={} | overrun out={} ref={} | estimado out={:+.1} ppm ref={:+.1} ppm ({:.1} s CPU)",
            s.out_under, s.ref_under, s.out_over, s.ref_over, s.out_ppm, s.ref_ppm, t.elapsed().as_secs_f64()
        );
        println!(
            "     buffer saída (visto pelo CABLE): média {:.2} ms, desvio {:.2}, min {:.2}, max {:.2}, 1os 5 min {:.2} / últimos 5 min {:.2} | referência: média {:.2} ms, desvio {:.2}, min {:.2}, max {:.2}",
            s.out_lat.0, s.out_lat.1, s.out_lat.2, s.out_lat.3, s.out_start_end.0, s.out_start_end.1, s.ref_lat.0, s.ref_lat.1, s.ref_lat.2, s.ref_lat.3
        );
        ok &= s.out_under == 0 && s.out_over == 0 && s.ref_under == 0 && s.ref_over == 0;
        ok &= (s.out_start_end.0 - s.out_start_end.1).abs() < 1.0 && s.out_lat.1 < 5.0 && s.out_lat.2 > 5.0;
        // Sinal do estimado: positivo = consumidor mais rápido (saída) / produtor mais rápido (referência).
        ok &= (s.out_ppm - o).abs() < 30.0 && (s.ref_ppm + r).abs() < 30.0 || (o == 0.0 && r == 0.0);
    }
    assert!(ok, "falhou: ver números acima");
}
