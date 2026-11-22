mod common;
use common::*;
use micclean::dsp::{Pipeline, build_chain};
use micclean::fx::biquad::{BqParams, BqType, Width};
use micclean::fx::chain::{BlockCfg, Builder, Kind, Preset};
use micclean::fx::vst::{VstIo, VstPlugin};
use micclean::{FRAME, SR};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

const DIR: &str = r"C:\Program Files\EqualizerAPO\VSTPlugins";

#[test]
fn v1_vst_e_cadeia_sem_alocar() {
    let comp = Path::new(DIR).join("Lisp x64.dll");
    let xcomp = Path::new(DIR).join("Black Box Analog Design HG-2MS.dll");
    println!();
    // Lisp (de-esser): parâmetros por nome
    let p = VstPlugin::load(&comp).expect("lisp");
    let (chunk, params) = p.get_state();
    println!("[V1] {} | in {} out {} | editor {} | chunk {} | {} parâmetros", p.name, p.n_in, p.n_out, p.has_editor, chunk.is_some(), params.len());
    println!("  parâmetros: {:?}", params.keys().collect::<Vec<_>>());
    let mut want = BTreeMap::new();
    if chunk.is_none() {
        let k = params.keys().next().unwrap().clone();
        want.insert(k.clone(), 0.37f32);
        p.set_state(None, &want);
        let (_, got) = p.get_state();
        println!("  {k} depois de set_state: {:?}", got.get(&k));
        assert!((got[&k] - 0.37).abs() < 0.02);
    }
    let mut io = VstIo::new(&p);
    let mut x = white(FRAME, 0.5, 3);
    p.process(&mut io, &mut x);
    assert!(x.iter().all(|v| v.is_finite()) && !p.failed.load(Relaxed));
    // HG-2MS: chunk base64 (o som.txt do usuário usa esse plugin)
    let x2 = VstPlugin::load(&xcomp).expect("hg-2ms");
    let (c0, _) = x2.get_state();
    let ch = c0.expect("HG-2MS usa chunk");
    x2.set_state(Some(&ch), &BTreeMap::new());
    let (back, _) = x2.get_state();
    println!("[V1] {} chunk de {} bytes volta igual: {}", x2.name, ch.len() * 3 / 4, back.as_deref() == Some(ch.as_str()));
    let ch = ch.as_str();

    // cadeia completa com todos os tipos de bloco, contando alocações por frame
    let pr = Preset {
        name: "t".into(),
        blocks: vec![
            BlockCfg { id: 1, enabled: true, kind: Kind::Aec { tuning: "equilibrado".into() } },
            BlockCfg { id: 2, enabled: true, kind: Kind::Denoise { atten_db: 100.0, post_filter: 0.0 } },
            BlockCfg { id: 3, enabled: true, kind: Kind::Preamp { gain_db: 3.5 } },
            BlockCfg {
                id: 4,
                enabled: true,
                kind: Kind::Filter { p: BqParams { kind: BqType::HighShelf, freq: 1137.71, gain: -6.0, width: 4.1578, width_kind: Width::Slope, corner: false } },
            },
            BlockCfg { id: 5, enabled: true, kind: Kind::GraphicEq { mode: Default::default(), nodes: vec![(100.0, -3.0), (5000.0, 2.0)] } },
            BlockCfg { id: 6, enabled: true, kind: Kind::Loudness { ref_level: -99.0, ref_offset: 38.0, attenuation: 1.0 } },
            BlockCfg { id: 7, enabled: true, kind: Kind::Delay { ms: 5.0 } },
            BlockCfg { id: 8, enabled: true, kind: Kind::Vst { library: comp.to_string_lossy().into(), chunk: None, params: want.clone() } },
            BlockCfg { id: 9, enabled: true, kind: Kind::Vst { library: xcomp.to_string_lossy().into(), chunk: Some(ch.into()), params: BTreeMap::new() } },
            BlockCfg { id: 10, enabled: true, kind: Kind::Limiter { ceiling_db: -1.5, release_ms: 50.0 } },
        ],
    };
    let mut cache = BTreeMap::new();
    let mut b = Builder { vst: &mut cache, vol_db: -20.0, base: Path::new(".") };
    let (chain, errs) = build_chain(&pr, &mut b);
    assert!(errs.is_empty(), "{errs:?}");
    let mut pipe = Pipeline::new(&pr, chain).unwrap();
    let sig = testdata("near_en.wav");
    let refz = [0.0f32; FRAME];
    let mut f = [0.0f32; FRAME];
    let mut worst = 0u64;
    let frames = sig.len() / FRAME - 1;
    for i in 0..200 {
        f.copy_from_slice(&sig[i * FRAME..(i + 1) * FRAME]);
        pipe.process(&mut f, &refz);
    }
    let a0 = ALLOCS.load(Relaxed);
    let t = std::time::Instant::now();
    for i in 200..frames {
        f.copy_from_slice(&sig[i * FRAME..(i + 1) * FRAME]);
        let tt = pipe.process(&mut f, &refz);
        worst = worst.max(tt.total_ns);
        assert!(f.iter().all(|v| v.is_finite()));
    }
    let n = (frames - 200) as f64;
    let per = (ALLOCS.load(Relaxed) - a0) as f64 / n;
    println!(
        "[V1] cadeia com 10 blocos (AEC, DFN, Preamp, Filter, GraphicEQ, Loudness, Delay, Lisp, HG-2MS, Limitador): {per:.2} alocações/frame, média {:.3} ms, pior {:.3} ms por frame de 10 ms",
        t.elapsed().as_secs_f64() * 1000.0 / n,
        worst as f64 / 1e6
    );
    // por bloco: só os blocos do AMA precisam ser 0; AEC3, DFN (tract) e plugins são código de terceiros
    let mut own = 0.0;
    for blk in &pr.blocks {
        let one = Preset { name: "u".into(), blocks: vec![blk.clone()] };
        let mut b = Builder { vst: &mut cache, vol_db: -20.0, base: Path::new(".") };
        let (chain, _) = build_chain(&one, &mut b);
        let mut pipe = Pipeline::new(&one, chain).unwrap();
        for i in 0..100 {
            f.copy_from_slice(&sig[i * FRAME..(i + 1) * FRAME]);
            pipe.process(&mut f, &refz);
        }
        let a0 = ALLOCS.load(Relaxed);
        for i in 100..400 {
            f.copy_from_slice(&sig[i * FRAME..(i + 1) * FRAME]);
            pipe.process(&mut f, &refz);
        }
        let per = (ALLOCS.load(Relaxed) - a0) as f64 / 300.0;
        println!("  {:<18} {per:.2} alocações/frame", blk.kind.title());
        if !matches!(blk.kind, Kind::Aec { .. } | Kind::Denoise { .. } | Kind::Vst { .. }) {
            own += per;
        }
    }
    assert_eq!(own, 0.0);
}
