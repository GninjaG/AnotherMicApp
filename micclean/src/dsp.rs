use crate::FRAME;
use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::Array2;
use sonora::config::{EchoCanceller, EchoCanceller3Config};
use sonora::{AudioProcessing, Config as ApmConfig, StreamConfig};
use std::time::Instant;
use crate::fx::chain::{BlockCfg, Builder, Chain, Cmd, Fx, Garbage, Kind, Preset, Slot};

pub struct Aec {
    apm: AudioProcessing,
    ref_out: Vec<f32>,
    out: Vec<f32>,
}

/// Ajuste "equilibrado": supressor confia mais no filtro linear, preserva mais a voz local em double-talk.
pub fn ec3_balanced() -> EchoCanceller3Config {
    let mut c = EchoCanceller3Config::default();
    let t = &mut c.suppressor.normal_tuning;
    t.mask_lf.enr_transparent = 0.6;
    t.mask_lf.enr_suppress = 0.8;
    t.mask_hf.enr_transparent = 0.14;
    t.mask_hf.enr_suppress = 0.2;
    let d = &mut c.suppressor.dominant_nearend_detection;
    d.enr_threshold = 0.5;
    d.snr_threshold = 10.0;
    d.trigger_threshold = 6;
    c.erle.max_l = 32.0;
    c.erle.max_h = 12.0;
    c.ep_strength.default_len = 0.3;
    c.ep_strength.nearend_len = 0.3;
    c
}

impl Aec {
    pub fn new() -> Self {
        Self::with_config(true, Some(ec3_balanced()))
    }

    pub fn from_tuning(tuning: &str) -> Self {
        match tuning {
            "webrtc" => Self::with_config(true, None),
            _ => Self::new(),
        }
    }

    pub fn with_hpf(hpf: bool) -> Self {
        Self::with_config(hpf, None)
    }

    pub fn with_config(hpf: bool, ec3: Option<EchoCanceller3Config>) -> Self {
        let ec = EchoCanceller { enforce_high_pass_filtering: hpf, ..Default::default() };
        let cfg = ApmConfig { echo_canceller: Some(ec), ..Default::default() };
        let sc = StreamConfig::new(crate::SR as u32, 1);
        let mut b = AudioProcessing::builder().config(cfg).capture_config(sc).render_config(sc);
        if let Some(c) = ec3 {
            b = b.echo_canceller3_config(c);
        }
        let apm = b.build();
        Self { apm, ref_out: vec![0.0; FRAME], out: vec![0.0; FRAME] }
    }

    pub fn process(&mut self, mic: &mut [f32], reference: &[f32]) {
        let _ = self.apm.process_render_f32(&[reference], &mut [&mut self.ref_out[..]]);
        if self.apm.process_capture_f32(&[&mic[..]], &mut [&mut self.out[..]]).is_ok() {
            mic.copy_from_slice(&self.out);
        }
    }

    /// (atraso estimado ms, ERLE dB). Usar fora do caminho quente.
    pub fn metrics(&self) -> (Option<i32>, Option<f64>) {
        let s = self.apm.statistics();
        (s.delay_ms.or(s.delay_median_ms), s.echo_return_loss_enhancement)
    }
}

impl Default for Aec {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Dfn {
    m: DfTract,
    inb: Array2<f32>,
    outb: Array2<f32>,
}

impl Dfn {
    pub fn new(atten_limit_db: f32, post_filter_beta: f32) -> Result<Self, String> {
        Self::with_model(atten_limit_db, post_filter_beta, false)
    }

    /// `ll`: modelo DeepFilterNet3 de baixa latência (sem lookahead, 20 ms a menos).
    pub fn with_model(atten_limit_db: f32, post_filter_beta: f32, ll: bool) -> Result<Self, String> {
        static LL: &[u8] = include_bytes!("../models/DeepFilterNet3_ll_onnx.tar.gz");
        let rp = RuntimeParams::default_with_ch(1)
            .with_atten_lim(atten_limit_db)
            .with_post_filter(post_filter_beta.max(0.0));
        let params = if ll { DfParams::from_bytes(LL).map_err(|e| format!("{e:#}"))? } else { DfParams::default() };
        let m = DfTract::new(params, &rp).map_err(|e| format!("{e:#}"))?;
        assert_eq!(m.hop_size, FRAME);
        Ok(Self { m, inb: Array2::zeros((1, FRAME)), outb: Array2::zeros((1, FRAME)) })
    }

    pub fn set_atten(&mut self, db: f32) {
        self.m.set_atten_lim(db);
    }

    /// Atraso algorítmico em amostras.
    pub fn delay(&self) -> usize {
        self.m.fft_size - self.m.hop_size + self.m.lookahead * self.m.hop_size
    }

    pub fn process(&mut self, x: &mut [f32]) {
        self.inb.as_slice_mut().unwrap().copy_from_slice(x);
        if self.m.process(self.inb.view(), self.outb.view_mut()).is_ok() {
            x.copy_from_slice(self.outb.as_slice().unwrap());
        }
    }
}

pub struct Limiter {
    env: f32,
    rel: f32,
    pub ceil: f32,
}

impl Limiter {
    pub fn new() -> Self {
        let rel = (-1.0 / (0.05 * crate::SR as f32)).exp();
        Self { env: 0.0, rel, ceil: 0.841 }
    }

    /// Retorna true se atenuou.
    pub fn process(&mut self, x: &mut [f32], gain: f32) -> bool {
        let mut limited = false;
        for s in x.iter_mut() {
            let v = *s * gain;
            let a = v.abs();
            self.env = if a > self.env { a } else { self.rel * self.env + (1.0 - self.rel) * a };
            if self.env > self.ceil {
                *s = v * (self.ceil / self.env);
                limited = true;
            } else {
                *s = v;
            }
        }
        limited
    }
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Times {
    pub aec_ns: u64,
    pub dfn_ns: u64,
    pub out_ns: u64,
    pub total_ns: u64,
    pub nan: bool,
    pub limited: bool,
}

/// Remove offset DC (passa-altas de 1ª ordem, ~10 Hz).
#[derive(Default)]
pub struct DcBlock {
    x1: f64,
    y1: f64,
}

impl DcBlock {
    pub fn process(&mut self, x: &mut [f32]) {
        const R: f64 = 1.0 - 2.0 * std::f64::consts::PI * 10.0 / crate::SR as f64;
        for v in x.iter_mut() {
            let y = *v as f64 - self.x1 + R * self.y1;
            self.x1 = *v as f64;
            self.y1 = y;
            *v = y as f32;
        }
    }
}

pub struct Pipeline {
    pub dc: DcBlock,
    pub aec: Aec,
    pub dfn: Dfn,
    pub chain: Box<Chain>,
    pub bypass: bool,
    pub mute: bool,
    tuning: String,
    atten: f32,
    post: f32,
    pub ll: bool,
    refb: Vec<f32>,
    pub resets: u64,
}

fn sanitize(x: &mut [f32]) -> bool {
    let mut bad = false;
    for v in x.iter_mut() {
        if !v.is_finite() {
            *v = 0.0;
            bad = true;
        }
    }
    bad
}

/// Cadeia equivalente ao config antigo (testes e mctools).
pub fn legacy_preset(c: &crate::config::Config) -> Preset {
    let mut b = Vec::new();
    let mut push = |enabled: bool, kind: Kind| {
        let id = b.len() as u32 + 1;
        b.push(BlockCfg { id, enabled, kind })
    };
    let aec = Kind::Aec { tuning: c.aec.tuning.clone() };
    let dfn = Kind::Denoise { atten_db: c.denoise.atten_limit_db, post_filter: c.denoise.post_filter_beta };
    if c.denoise.before_aec {
        push(c.denoise.enabled, dfn);
        push(c.aec.enabled, aec);
    } else {
        push(c.aec.enabled, aec);
        push(c.denoise.enabled, dfn);
    }
    if c.output.enabled && c.output.gain_db != 0.0 {
        push(true, Kind::Preamp { gain_db: c.output.gain_db as f64 });
    }
    if c.output.enabled && c.output.limiter {
        push(true, Kind::Limiter { ceiling_db: -1.5, release_ms: 50.0 });
    }
    Preset { name: "config".into(), blocks: b }
}

/// Monta a cadeia de execução (aloca; fora da thread de áudio). Retorna erros por bloco.
pub fn build_chain(p: &Preset, bld: &mut Builder) -> (Box<Chain>, Vec<(u32, String)>) {
    let mut errs = Vec::new();
    let mut slots = Vec::with_capacity(p.blocks.len() + 8);
    for b in &p.blocks {
        let (fx, e) = bld.build(b);
        if let Some(e) = e {
            errs.push((b.id, e));
        }
        slots.push(Slot { id: b.id, enabled: b.enabled, fx });
    }
    (Box::new(Chain { slots }), errs)
}

fn push_bin(g: Garbage, bin: &mut rtrb::Producer<Garbage>) {
    if let Err(rtrb::PushError::Full(g)) = bin.push(g) {
        std::mem::forget(g);
    }
}

impl Pipeline {
    pub fn from_config(c: &crate::config::Config) -> Result<Self, String> {
        let p = legacy_preset(c);
        let mut vst = Default::default();
        let mut bld = Builder { vst: &mut vst, vol_db: 0.0, base: std::path::Path::new(".") };
        let (chain, _) = build_chain(&p, &mut bld);
        Self::new(&p, chain)
    }

    pub fn new(p: &Preset, chain: Box<Chain>) -> Result<Self, String> {
        Self::with_model(p, chain, false)
    }

    pub fn with_model(p: &Preset, chain: Box<Chain>, ll: bool) -> Result<Self, String> {
        let (mut tuning, mut atten, mut post) = ("equilibrado".to_string(), 100.0, 0.0);
        for b in &p.blocks {
            match &b.kind {
                Kind::Aec { tuning: t } => tuning = t.clone(),
                Kind::Denoise { atten_db, post_filter } => (atten, post) = (*atten_db, *post_filter),
                _ => {}
            }
        }
        Ok(Self {
            dc: DcBlock::default(),
            aec: Aec::from_tuning(&tuning),
            dfn: Dfn::with_model(atten, post, ll)?,
            chain,
            bypass: false,
            mute: false,
            tuning,
            atten,
            post,
            ll,
            refb: vec![0.0; FRAME],
            resets: 0,
        })
    }

    /// Troca o modelo do DeepFilterNet (aloca; só ao mudar o modo de latência).
    pub fn set_model(&mut self, ll: bool) {
        if ll == self.ll {
            return;
        }
        if let Ok(n) = Dfn::with_model(self.atten, self.post, ll) {
            self.dfn = n;
            self.ll = ll;
        }
    }

    pub fn set_atten(&mut self, db: f32) {
        self.atten = db;
        self.dfn.set_atten(db);
    }

    pub fn atten(&self) -> f32 {
        self.atten
    }

    pub fn has_aec(&self) -> bool {
        self.chain.slots.iter().any(|s| s.enabled && matches!(s.fx, Fx::Aec))
    }

    /// Aplica um comando da interface. Objetos substituídos vão para `bin` (liberados fora daqui).
    pub fn apply(&mut self, cmd: Cmd, bin: &mut rtrb::Producer<Garbage>) {
        match cmd {
            Cmd::SetChain(c) => {
                let old = std::mem::replace(&mut self.chain, c);
                push_bin(Garbage::Chain(old), bin);
            }
            Cmd::Enable(id, on) => {
                if let Some(s) = self.chain.slot(id) {
                    s.enabled = on;
                }
            }
            Cmd::Coefs(id, c) => {
                if let Some(Slot { fx: Fx::Bq(b), .. }) = self.chain.slot(id) {
                    b.c = c;
                }
            }
            Cmd::Gain(id, g) => {
                if let Some(Slot { fx: Fx::Gain(v), .. }) = self.chain.slot(id) {
                    *v = g;
                }
            }
            Cmd::Lim(id, c, r) => {
                if let Some(Slot { fx: Fx::Lim(l), .. }) = self.chain.slot(id) {
                    l.set(c, r);
                }
            }
            Cmd::Loud(id, c) => {
                if let Some(Slot { fx: Fx::Loud(l), .. }) = self.chain.slot(id) {
                    l.set(c);
                }
            }
            Cmd::Replace(id, mut fx) => {
                if let Some(s) = self.chain.slot(id) {
                    std::mem::swap(&mut s.fx, &mut *fx);
                }
                push_bin(Garbage::Fx(fx), bin);
            }
            Cmd::Atten(db) => self.set_atten(db),
            Cmd::AecTuning(t) => {
                // raro: recria o AEC (aloca)
                self.aec = Aec::from_tuning(&t);
                let old = std::mem::replace(&mut self.tuning, t);
                push_bin(Garbage::Str(old), bin);
            }
        }
    }

    pub fn process(&mut self, x: &mut [f32], reference: &[f32]) -> Times {
        let t0 = Instant::now();
        let mut t = Times::default();
        t.nan = sanitize(x);
        self.dc.process(x);
        self.refb.copy_from_slice(&reference[..FRAME]);
        t.nan |= sanitize(&mut self.refb);
        if !self.bypass {
            let Pipeline { aec, dfn, chain, refb, tuning, atten, post, resets, ll, .. } = self;
            for s in chain.slots.iter_mut().filter(|s| s.enabled) {
                match &mut s.fx {
                    Fx::Aec => {
                        let a = Instant::now();
                        aec.process(x, refb);
                        if sanitize(x) {
                            t.nan = true;
                            *aec = Aec::from_tuning(tuning);
                            *resets += 1;
                        }
                        t.aec_ns += a.elapsed().as_nanos() as u64;
                    }
                    Fx::Denoise => {
                        let a = Instant::now();
                        dfn.process(x);
                        if sanitize(x) {
                            t.nan = true;
                            if let Ok(n) = Dfn::with_model(*atten, *post, *ll) {
                                *dfn = n;
                            }
                            *resets += 1;
                        }
                        t.dfn_ns += a.elapsed().as_nanos() as u64;
                    }
                    Fx::Gain(g) => x.iter_mut().for_each(|v| *v *= *g),
                    Fx::Bq(b) => b.process(x),
                    Fx::Conv(c) => c.process(x),
                    Fx::Loud(l) => l.process(x),
                    Fx::Delay(d) => d.process(x),
                    Fx::Lim(l) => t.limited |= l.process(x),
                    Fx::Vst(p, io) => p.process(io, x),
                    Fx::Off => {}
                }
            }
            t.nan |= sanitize(x);
        }
        for v in x.iter_mut() {
            *v = v.clamp(-1.0, 1.0);
        }
        if self.mute {
            x.fill(0.0);
        }
        t.total_ns = t0.elapsed().as_nanos() as u64;
        t
    }
}
