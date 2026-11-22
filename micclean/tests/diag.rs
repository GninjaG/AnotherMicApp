mod common;
use common::*;
use micclean::SR;

#[test]
#[ignore]
fn diag_dt() {
    let reference = testdata("far_pt.wav");
    let near0 = testdata("near_en.wav");
    let n = reference.len();
    let start = 12 * SR;
    let mut near = vec![0.0f32; n];
    let len = near0.len().min(n - start);
    near[start..start + len].copy_from_slice(&near0[..len]);
    let (a, b) = (start, start + len);
    for (label, eg, refon) in [("só voz local, ref mudo", 0.0f32, false), ("só voz local, ref tocando sem eco", 0.0, true), ("eco -10 dB", 0.17, true), ("eco -3.6 dB", 0.5, true)] {
        let echo = convolve(&reference, &room_ir(70.0, eg.max(1e-6), 3));
        let r: Vec<f32> = if refon { reference.clone() } else { vec![0.0; n] };
        let mic: Vec<f32> = (0..n).map(|i| echo[i] + near[i]).collect();
        let out = run_aec(&mic, &r);
        let d = find_delay(&near[a..b], &out[a..b], 960);
        let c = pearson(&near[a..b - d], &out[a + d..b]);
        println!("{label}: delay={d} corr={c:.3} ganho saída/voz={:.1} dB", db(energy(&out[a + d..b]) / energy(&near[a..b - d])));
    }
}

#[test]
#[ignore]
fn diag_hpf() {
    let near = testdata("near_en.wav");
    let n = near.len() / 480 * 480;
    for hpf in [true, false] {
        let mut aec = micclean::dsp::Aec::with_hpf(hpf);
        let mut out = vec![0.0f32; n];
        let z = [0.0f32; 480];
        for i in (0..n).step_by(480) {
            let mut f = [0.0f32; 480];
            f.copy_from_slice(&near[i..i + 480]);
            aec.process(&mut f, &z);
            out[i..i + 480].copy_from_slice(&f);
        }
        let d = find_delay(&near, &out, 960);
        let c = pearson(&near[..n - d], &out[d..n]);
        // correlação de envelope (20 ms)
        let env = |x: &[f32]| -> Vec<f32> { x.chunks(960).map(|c| (energy(c) / 960.0).sqrt() as f32).collect() };
        let ce = pearson(&env(&near[..n - d]), &env(&out[d..n]));
        println!("hpf={hpf}: delay={d} corr_onda={c:.3} corr_envelope={ce:.3}");
        // corr com o próprio sinal passado por passa-alta simples de 80 Hz
    }
}

fn variant(k: usize) -> Option<sonora::config::EchoCanceller3Config> {
    let mut c = sonora::config::EchoCanceller3Config::default();
    match k {
        0 => return None,
        1 => c.suppressor.use_subband_nearend_detection = true,
        2 => {
            let d = &mut c.suppressor.dominant_nearend_detection;
            d.enr_threshold = 0.5;
            d.snr_threshold = 10.0;
            d.hold_duration = 25;
            d.trigger_threshold = 6;
        }
        3 => {
            let t = &mut c.suppressor.normal_tuning;
            t.mask_lf.enr_transparent = 0.6; t.mask_lf.enr_suppress = 0.8;
            t.mask_hf.enr_transparent = 0.14; t.mask_hf.enr_suppress = 0.2;
            let d = &mut c.suppressor.dominant_nearend_detection;
            d.enr_threshold = 0.5; d.snr_threshold = 10.0; d.trigger_threshold = 6;
        }
        4 | 5 | 6 | 7 | 8 => {
            let t = &mut c.suppressor.normal_tuning;
            t.mask_lf.enr_transparent = 0.6; t.mask_lf.enr_suppress = 0.8;
            t.mask_hf.enr_transparent = 0.14; t.mask_hf.enr_suppress = 0.2;
            let d = &mut c.suppressor.dominant_nearend_detection;
            d.enr_threshold = 0.5; d.snr_threshold = 10.0; d.trigger_threshold = 6;
            let (l, h) = match k { 4 => (8.0, 3.0), 5 => (16.0, 6.0), _ => (32.0, 12.0) };
            if k >= 7 { c.ep_strength.default_len = 0.3; c.ep_strength.nearend_len = 0.3; }
            if k == 8 { c.ep_strength.default_gain = 0.7; }
            c.erle.max_l = l; c.erle.max_h = h;
            if k == 6 && k < 7 { c.ep_strength.default_len = 0.5; c.ep_strength.nearend_len = 0.5; }
        }
        _ => unreachable!(),
    }
    Some(c)
}

fn run_aec_v(mic: &[f32], r: &[f32], k: usize) -> Vec<f32> {
    let mut aec = micclean::dsp::Aec::with_config(true, variant(k));
    let n = mic.len() / 480 * 480;
    let mut out = vec![0.0f32; mic.len()];
    for i in (0..n).step_by(480) {
        let mut f = [0.0f32; 480];
        f.copy_from_slice(&mic[i..i + 480]);
        aec.process(&mut f, &r[i..i + 480]);
        out[i..i + 480].copy_from_slice(&f);
    }
    out
}

#[test]
#[ignore]
fn diag_variants() {
    let reference = testdata("far_pt.wav");
    let near0 = testdata("near_en.wav");
    let n = reference.len();
    let start = 12 * SR;
    let mut near = vec![0.0f32; n];
    let len = near0.len().min(n - start);
    near[start..start + len].copy_from_slice(&near0[..len]);
    let (a, b) = (start, start + len);
    for k in [6usize, 7, 8] {
        let near_lin = run_aec_v(&near, &vec![0.0; n], k);
        for g in [0.5f32, 0.17, 0.09] {
            let echo = convolve(&reference, &room_ir(70.0, g, 3));
            let mic: Vec<f32> = (0..n).map(|i| echo[i] + near[i]).collect();
            let out = run_aec_v(&mic, &reference, k);
            let corr = pearson(&near_lin[a..b], &out[a..b]);
            let w = SR / 50;
            let (mut run, mut longest, mut cut, mut act) = (0, 0, 0, 0);
            for s in (a..b - w).step_by(w) {
                let ln = rms_db(&near_lin[s..s + w]);
                if ln < -40.0 { run = 0; continue; }
                act += 1;
                if rms_db(&out[s..s + w]) < ln - 15.0 { run += 1; cut += 1; longest = longest.max(run); } else { run = 0; }
            }
            // ERLE single-talk antes da voz local
            let erle = db(energy(&mic[5 * SR..start]) / energy(&out[5 * SR..start]));
            // eco residual durante DT: saída - voz local
            let res: Vec<f32> = (a..b).map(|i| out[i] - near_lin[i]).collect();
            let erle_dt = db(energy(&echo[a..b]) / energy(&res));
            println!("v{k} eco {:+.1} dB: corr={corr:.3} corte_max={}ms cortes={:.1}% ERLE_st={erle:.1} ERLE_dt={erle_dt:.1}", db(energy(&echo[a..b]) / energy(&near[a..b])), longest * 20, 100.0 * cut as f64 / act as f64);
        }
    }
}

#[test]
#[ignore]
fn diag_dfn_ceiling() {
    let clean = testdata("clean_pt.wav");
    let n = clean.len() / 480 * 480;
    let clean = &clean[..n];
    for (lim, pf) in [(100.0f32, 0.0f32), (30.0, 0.0), (100.0, 0.02)] {
        let mut d = micclean::dsp::Dfn::new(lim, pf).unwrap();
        let mut out = vec![0.0f32; n];
        for i in (0..n).step_by(480) {
            let mut f = [0.0f32; 480];
            f.copy_from_slice(&clean[i..i + 480]);
            d.process(&mut f);
            out[i..i + 480].copy_from_slice(&f);
        }
        for od in [1440usize, 1439, 1441, 960, 1920] {
            println!("lim={lim} pf={pf} delay={od}: SI-SDR(clean->dfn) = {:.1} dB", si_sdr(&clean[..n - od], &out[od..]));
        }
        let fd = find_delay(clean, &out, 4800);
        println!("  find_delay={fd}");
    }
}
