mod common;
use common::*;
use micclean::fx::biquad::{Biquad, BqParams, BqType, Coefs, Width};
use micclean::fx::chain::{BlockCfg, Kind, Preset};
use micclean::fx::conv::Conv;
use micclean::fx::{eapo, geq};
use micclean::{FRAME, SR};
use std::path::Path;

fn sine(f: f64, n: usize) -> Vec<f32> {
    (0..n).map(|i| (0.25 * (2.0 * std::f64::consts::PI * f * i as f64 / SR as f64).sin()) as f32).collect()
}

fn gain_through(mut f: impl FnMut(&mut [f32]), freq: f64) -> f64 {
    let n = SR * 2;
    let mut x = sine(freq, n);
    let r0 = rms_db(&x[SR..]);
    for c in x.chunks_mut(FRAME) {
        f(c);
    }
    rms_db(&x[SR..]) - r0
}

#[test]
fn f1_biquads_batem_com_a_formula() {
    let cases = [
        BqParams { kind: BqType::Peak, freq: 1000.0, gain: 6.0, width: 1.0, width_kind: Width::Q, corner: false },
        BqParams { kind: BqType::HighShelf, freq: 1137.71, gain: -6.0, width: 4.1578, width_kind: Width::Slope, corner: false },
        BqParams { kind: BqType::LowShelf, freq: 200.0, gain: 4.0, width: 0.0, width_kind: Width::Q, corner: true },
        BqParams { kind: BqType::HighPass, freq: 120.0, gain: 0.0, width: 0.0, width_kind: Width::Q, corner: false },
        BqParams { kind: BqType::Notch, freq: 60.0, gain: 0.0, width: 0.0, width_kind: Width::Q, corner: false },
    ];
    println!();
    for p in cases {
        let c = Coefs::new(&p, SR as f64);
        for f in [50.0, 120.0, 1000.0, 5000.0, 12000.0] {
            let mut b = Biquad::new(c);
            let m = gain_through(|x| b.process(x), f);
            let t = c.gain_at(f, SR as f64);
            println!("[F1] {:?} {} Hz: medido {m:+.2} dB, fórmula {t:+.2} dB", p.kind, f);
            assert!((m - t).abs() < 0.15 || t < -30.0, "{p:?} em {f}");
        }
    }
    let hs = Coefs::new(&cases[1], SR as f64);
    assert!((hs.gain_at(15000.0, SR as f64) + 6.0).abs() < 0.3);
}

#[test]
fn f2_convolucao_igual_a_direta() {
    let x = white(SR, 0.3, 4);
    let ir = room_ir(5.0, 0.5, 7)[..3000].to_vec();
    let want = convolve(&x, &ir);
    let mut c = Conv::new(&ir);
    let mut y = x.clone();
    for f in y.chunks_mut(FRAME) {
        c.process(f);
    }
    let err = y.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    println!("\n[F2] convolução particionada vs direta (IR {} taps): erro máx {err:.2e}", ir.len());
    assert!(err < 1e-4);
    let mut id = Conv::new(&[1.0]);
    let mut z = x[..FRAME].to_vec();
    id.process(&mut z);
    assert!(z.iter().zip(&x).all(|(a, b)| (a - b).abs() < 1e-6));
}

#[test]
fn f3_graphiceq_segue_os_nos() {
    let nodes = vec![(20.0, -6.0), (100.0, -6.0), (1000.0, 0.0), (8000.0, 6.0), (20000.0, 6.0)];
    let t = std::time::Instant::now();
    let ir = geq::design(&nodes, SR as f64);
    let dt = t.elapsed().as_secs_f64() * 1000.0;
    let mut c = Conv::new(&ir);
    println!("\n[F3] GraphicEQ: FIR de {} taps em {dt:.1} ms", ir.len());
    for f in [60.0, 316.0, 1000.0, 2828.0, 10000.0] {
        let m = gain_through(|x| c.process(x), f);
        let want = geq::gain_at(&nodes, f);
        println!("  {f} Hz: medido {m:+.2} dB, alvo {want:+.2} dB");
        assert!((m - want).abs() < 0.5);
    }
}

#[test]
fn f4_importa_e_exporta_eapo() {
    let dir = out_path("eapo");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("teste.txt");
    std::fs::write(
        &f,
        "Device: CABLE Output\nPreamp: 3.5 dB\nFilter: ON HSC 4.1578 dB Fc 1137.71 Hz Gain -6 dB\nFilter 2: OFF PK Fc 250 Hz Gain -3 dB Q 1.41\n\
         GraphicEQ: 25 -2.8; 40 -2; 63 -0.4\nDelay: 12 ms\nLoudnessCorrection: State 1 ReferenceLevel -99 ReferenceOffset 38 Attenuation 1.0\n\
         VSTPlugin: Library \"C:\\x\\reacomp.dll\" Thresh 0.07 \"RMS size\" 0.005\nInclude: extra.txt\n",
    )
    .unwrap();
    std::fs::write(dir.join("extra.txt"), "Filter: ON HP Fc 80 Hz\n").unwrap();
    let (b, warn) = eapo::import(&f).unwrap();
    println!("\n[F4] importados {} blocos, avisos: {warn:?}", b.len());
    let kinds: Vec<&str> = b.iter().map(|x| x.kind.title()).collect();
    println!("  {kinds:?}");
    assert_eq!(b.len(), 8);
    assert!(!b[2].enabled);
    match &b[1].kind {
        Kind::Filter { p } => assert!(p.kind == BqType::HighShelf && p.width_kind == Width::Slope && (p.width - 4.1578).abs() < 1e-9 && !p.corner),
        k => panic!("{k:?}"),
    }
    match &b[6].kind {
        Kind::Vst { params, .. } => assert_eq!(params.get("RMS size"), Some(&0.005)),
        k => panic!("{k:?}"),
    }
    let txt = eapo::export(&b, Path::new("."));
    println!("{txt}");
    let g = dir.join("volta.txt");
    std::fs::write(&g, &txt).unwrap();
    let (b2, _) = eapo::import(&g).unwrap();
    // o filtro desligado volta comentado, então some na reimportação
    assert_eq!(b2.len(), b.len() - 1);
    assert_eq!(b2[1].kind, b[1].kind);
}

#[test]
fn f5_preset_toml_ida_e_volta() {
    let mut p = Preset::default_chain("Teste");
    p.blocks.push(BlockCfg { id: 9, enabled: false, kind: Kind::GraphicEq { mode: Default::default(), nodes: vec![(25.0, 1.0)] } });
    let s = toml::to_string_pretty(&p).unwrap();
    let q: Preset = toml::from_str(&s).unwrap();
    assert_eq!(p, q);
}
