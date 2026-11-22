mod common;
use common::*;
use micclean::SR;

fn far() -> Vec<f32> {
    testdata("far_pt.wav")
}

#[test]
fn t1_aec_eco_simples() {
    let reference = far();
    println!("\n[T1] AEC eco simples (ref = fala TTS {:.1} s)", reference.len() as f64 / SR as f64);
    let mut worst = f64::MAX;
    for (i, delay) in [30.0, 60.0, 90.0, 120.0].into_iter().enumerate() {
        let h = room_ir(delay, 0.5, 10 + i as u64);
        let echo = convolve(&reference, &h);
        let noise = white(echo.len(), 0.001, 99);
        let mic: Vec<f32> = echo.iter().zip(&noise).map(|(a, b)| a + b).collect();
        let out = run_aec(&mic, &reference);
        let s = 10 * SR;
        let n = out.len() / 480 * 480;
        let erle = db(energy(&mic[s..n]) / energy(&out[s..n]));
        let w = windowed_erle(&mic, &out, &echo, SR / 2);
        let wv: Vec<f64> = w.iter().filter(|(t, _)| *t >= 10.0).filter_map(|x| x.1).collect();
        let mut sorted = wv.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p10 = sorted[sorted.len() / 10];
        println!(
            "  atraso {delay:>5.0} ms: ERLE global (t>=10s) = {erle:5.1} dB | janelas 0,5 s: mediana {:5.1} dB, p10 {:5.1} dB | eco {:.1} dBFS",
            sorted[sorted.len() / 2],
            p10,
            rms_db(&echo)
        );
        if delay == 60.0 {
            micclean::wav::write_mono(&out_path("t1_mic.wav"), &mic, SR as u32).unwrap();
            micclean::wav::write_mono(&out_path("t1_out.wav"), &out, SR as u32).unwrap();
        }
        worst = worst.min(erle);
    }
    assert!(worst >= 20.0, "ERLE {worst:.1} dB < 20 dB");
}

#[test]
fn t2_aec_fala_dos_dois_lados() {
    let reference = far();
    let near0 = testdata("near_en.wav");
    let n = reference.len();
    let start = 12 * SR;
    let mut near = vec![0.0f32; n];
    let len = near0.len().min(n - start);
    near[start..start + len].copy_from_slice(&near0[..len]);
    // Referência justa: voz local pelo mesmo AEC sem eco (passa-alta + banco de filtros do AEC3).
    let near_lin = run_aec(&near, &vec![0.0; n]);
    let (a, b) = (start, start + len);
    let d0 = find_delay(&near[a..b], &near_lin[a..b], 960);
    println!("
[T2] AEC fala dos dois lados ({:.1} s de double-talk, voz local en-US + eco pt-BR, atraso do AEC = {d0} amostras)", len as f64 / SR as f64);
    let mut fails = Vec::new();
    for g in [0.5f32, 0.17, 0.09] {
        let echo = convolve(&reference, &room_ir(70.0, g, 3));
        let noise = white(n, 0.001, 5);
        let mic: Vec<f32> = (0..n).map(|i| echo[i] + near[i] + noise[i]).collect();
        let out = run_aec(&mic, &reference);
        let enr = db(energy(&echo[a..b]) / energy(&near[a..b]));
        if g == 0.5 {
            micclean::wav::write_mono(&out_path("t2_mic.wav"), &mic, SR as u32).unwrap();
            micclean::wav::write_mono(&out_path("t2_out.wav"), &out, SR as u32).unwrap();
        }
        let (o, nr) = (&out[a..b], &near_lin[a..b]);
        let corr = pearson(nr, o);
        let corr_mic = pearson(nr, &run_aec(&mic, &vec![0.0; n])[a..b]);
        let w = SR / 50;
        let (mut run, mut longest, mut cut, mut act) = (0usize, 0usize, 0usize, 0usize);
        for s in (0..nr.len() - w).step_by(w) {
            let ln = rms_db(&nr[s..s + w]);
            if ln < -40.0 {
                run = 0;
                continue;
            }
            act += 1;
            if rms_db(&o[s..s + w]) < ln - 15.0 {
                run += 1;
                cut += 1;
                longest = longest.max(run);
            } else {
                run = 0;
            }
        }
        let st = db(energy(&mic[5 * SR..start]) / energy(&out[5 * SR..start]));
        println!(
            "  eco/voz {enr:+5.1} dB: correlação {corr:.3} (sem cancelar {corr_mic:.3}) | maior corte {} ms | cortes {:.1}% do tempo com voz | ERLE só eco antes {st:.1} dB",
            longest * 20,
            100.0 * cut as f64 / act as f64
        );
        if corr < 0.85 || longest * 20 > 200 {
            fails.push(format!("eco {enr:+.1} dB: corr {corr:.3}, corte {} ms", longest * 20));
        }
    }
    assert!(fails.is_empty(), "{fails:?}");
}

#[test]
fn t3_aec_mudanca_de_atraso() {
    let reference = far();
    let n = reference.len();
    let sw = 25 * SR;
    let e1 = convolve(&reference, &room_ir(40.0, 0.5, 7));
    let e2 = convolve(&reference, &room_ir(110.0, 0.5, 8));
    let echo: Vec<f32> = (0..n).map(|i| if i < sw { e1[i] } else { e2[i] }).collect();
    let noise = white(n, 0.001, 11);
    let mic: Vec<f32> = (0..n).map(|i| echo[i] + noise[i]).collect();
    let out = run_aec(&mic, &reference);
    let w = windowed_erle(&mic, &out, &echo, SR / 4);
    let before: Vec<f64> = w.iter().filter(|(t, _)| (10.0..25.0).contains(t)).filter_map(|x| x.1).collect();
    let mb = before.iter().sum::<f64>() / before.len() as f64;
    let after: Vec<&(f64, Option<f64>)> = w.iter().filter(|(t, _)| *t >= 25.0).collect();
    let mut rec = None;
    for i in 0..after.len().saturating_sub(4) {
        let ok = after[i..i + 4].iter().all(|(_, e)| e.map(|v| v >= 15.0).unwrap_or(true))
            && after[i..i + 4].iter().any(|(_, e)| e.is_some());
        if ok {
            rec = Some(after[i].0 - 25.0);
            break;
        }
    }
    let tail: Vec<f64> = w.iter().filter(|(t, _)| *t >= 40.0).filter_map(|x| x.1).collect();
    let mt = tail.iter().sum::<f64>() / tail.len() as f64;
    let min_after = after.iter().take(8).filter_map(|x| x.1).fold(f64::MAX, f64::min);
    println!("\n[T3] AEC mudança de atraso 40 -> 110 ms em t=25 s");
    println!("  ERLE médio antes: {mb:.1} dB | pior janela logo após a troca: {min_after:.1} dB | médio t>=40 s: {mt:.1} dB");
    match rec {
        Some(t) => println!("  tempo para voltar a >=15 dB (1 s seguido): {:.2} s", t),
        None => println!("  NÃO voltou a >=15 dB"),
    }
    assert!(rec.map(|t| t <= 10.0).unwrap_or(false), "não recuperou em 10 s");
}
