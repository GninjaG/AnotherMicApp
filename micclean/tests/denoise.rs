mod common;
use common::*;
use micclean::SR;

#[test]
fn t4_deepfilternet() {
    let clean = testdata("clean_pt.wav");
    let n = clean.len() / 480 * 480;
    let floor = white(n, 1.7e-4, 77);
    let cf: Vec<f32> = (0..n).map(|i| clean[i] + floor[i]).collect();
    let clean = &cf[..];
    let d = micclean::dsp::Dfn::new(100.0, 0.0).unwrap().delay();
    println!("\n[T4] DeepFilterNet3 ({:.1} s de fala, atraso algorítmico {} amostras = {:.1} ms)", n as f64 / SR as f64, d, d as f64 * 1000.0 / SR as f64);
    let ceiling = si_sdr(&clean[..n - d], &run_dfn(clean)[d..]);
    println!("  teto: SI-SDR da fala LIMPA após o DFN = {ceiling:.1} dB (distorção própria do modelo)");
    println!("  {:<9} {:>4} | {:>9} {:>9} {:>7} | {:>12} {:>12} {:>8}", "ruído", "SNR", "SI-SDR in", "SI-SDR out", "ganho", "ruído pausas", "voz (perda)", "ΔSNR");
    let noises: Vec<(&str, Vec<f32>)> = vec![
        ("ventilad", fan_noise(n, 1)),
        ("teclado", keyboard_noise(n, 2)),
        ("vozes", babble(n)),
        ("misto", {
            let f = fan_noise(n, 3);
            let k = keyboard_noise(n, 4);
            let b = babble(n);
            (0..n).map(|i| f[i] + 0.5 * k[i] + 0.5 * b[i]).collect()
        }),
    ];
    // pausas = janelas de 20 ms onde a fala limpa está 50 dB abaixo do pico
    let w = SR / 50;
    let peak = clean.chunks(w).map(|c| rms_db(c)).fold(f64::MIN, f64::max);
    let pauses: Vec<usize> = (0..n / w).filter(|&k| rms_db(&clean[k * w..(k + 1) * w]) < peak - 50.0).collect();
    let speech: Vec<usize> = (0..n / w).filter(|&k| rms_db(&clean[k * w..(k + 1) * w]) > peak - 20.0).collect();
    let mut results = Vec::new();
    for (name, noise) in &noises {
        for snr in [0.0, 5.0, 10.0] {
            let nz = scale_to_snr(clean, noise, snr);
            let noisy: Vec<f32> = (0..n).map(|i| clean[i] + nz[i]).collect();
            let out = run_dfn(&noisy);
            let od = find_delay(clean, &out, 3 * 480);
            let o = &out[od..];
            let c = &clean[..n - od];
            let sdr_in = si_sdr(clean, &noisy);
            let sdr_out = si_sdr(c, o);
            let sel = |x: &[f32], idx: &[usize]| -> f64 {
                idx.iter().filter(|&&k| (k + 1) * w <= x.len()).map(|&k| energy(&x[k * w..(k + 1) * w])).sum()
            };
            let pin = sel(&noisy, &pauses);
            let pout = sel(o, &pauses);
            let sin = sel(&clean[..n - od], &speech);
            let sout = sel(o, &speech);
            let noise_red = db(pin / pout.max(1e-20));
            let speech_loss = db(sin / sout.max(1e-20));
            println!(
                "  {:<9} {:>4.0} | {:>8.1} {:>9.1} {:>+7.1} | {:>9.1} dB {:>9.1} dB {:>+6.1} dB",
                name, snr, sdr_in, sdr_out, sdr_out - sdr_in, noise_red, speech_loss, noise_red - speech_loss
            );
            if *name == "misto" && snr == 5.0 {
                micclean::wav::write_mono(&out_path("t4_noisy.wav"), &noisy, SR as u32).unwrap();
                micclean::wav::write_mono(&out_path("t4_out.wav"), &out, SR as u32).unwrap();
            }
            results.push((name.to_string(), snr, sdr_out - sdr_in, noise_red - speech_loss, od, sdr_in));
        }
    }
    let mean: f64 = results.iter().map(|r| r.2).sum::<f64>() / results.len() as f64;
    println!("  melhora média de SI-SDR: {mean:+.1} dB | atraso medido da saída: {} amostras", results[0].4);
    let mut fails = Vec::new();
    for r in &results {
        if r.3 <= 0.0 {
            fails.push(format!("{} {} dB: SNR não melhorou ({:+.1})", r.0, r.1, r.3));
        }
        // SI-SDR deve subir quando a entrada está 3 dB ou mais abaixo do teto do modelo (exceto vozes ao fundo, ver relatório)
        if r.0 != "vozes" && r.5 < ceiling - 3.0 && r.2 <= 0.0 {
            fails.push(format!("{} {} dB: SI-SDR não melhorou ({:+.1})", r.0, r.1, r.2));
        }
    }
    let bab: Vec<_> = results.iter().filter(|r| r.0 == "vozes").collect();
    println!("  LIMITAÇÃO vozes ao fundo: ΔSNR {:+.1}/{:+.1}/{:+.1} dB, SI-SDR {:+.1}/{:+.1}/{:+.1} dB (0/5/10 dB)", bab[0].3, bab[1].3, bab[2].3, bab[0].2, bab[1].2, bab[2].2);
    assert!(fails.is_empty(), "{fails:?}");
}
