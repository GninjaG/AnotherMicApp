use super::biquad::{BqParams, Biquad, Coefs};
use super::conv::{Conv, load_ir};
use super::dynamics::{Comp, CompParams, DeEss, DeEssParams};
use super::loudness::LoudCoefs;
use super::vst::{VstIo, VstPlugin};
use crate::{FRAME, SR};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GeqMode {
    #[default]
    Bands15,
    Bands31,
    Free,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Kind {
    Aec { tuning: String },
    Denoise { atten_db: f32, post_filter: f32 },
    Preamp { gain_db: f64 },
    Filter { p: BqParams },
    GraphicEq { mode: GeqMode, nodes: Vec<(f64, f64)> },
    Loudness { ref_level: f64, ref_offset: f64, attenuation: f64 },
    Convolution { file: String },
    Delay { ms: f64 },
    Limiter { ceiling_db: f64, release_ms: f64 },
    Compressor { p: CompParams },
    DeEsser { p: DeEssParams },
    Vst { library: String, chunk: Option<String>, params: BTreeMap<String, f32> },
}

impl Kind {
    pub fn title(&self) -> &'static str {
        match self {
            Kind::Aec { .. } => "Cancelamento de eco (AEC)",
            Kind::Denoise { .. } => "Redução de ruído (DeepFilterNet)",
            Kind::Preamp { .. } => "Preamp",
            Kind::Filter { .. } => "Filtro paramétrico",
            Kind::GraphicEq { .. } => "GraphicEQ",
            Kind::Loudness { .. } => "Loudness correction",
            Kind::Convolution { .. } => "Convolução",
            Kind::Delay { .. } => "Delay",
            Kind::Limiter { .. } => "Limitador",
            Kind::Compressor { .. } => "Compressor",
            Kind::DeEsser { .. } => "De-esser",
            Kind::Vst { .. } => "Plugin VST",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockCfg {
    pub id: u32,
    pub enabled: bool,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub blocks: Vec<BlockCfg>,
}

impl Preset {
    pub fn default_chain(name: &str) -> Preset {
        Preset {
            name: name.into(),
            blocks: vec![
                BlockCfg { id: 1, enabled: true, kind: Kind::Aec { tuning: "equilibrado".into() } },
                BlockCfg { id: 2, enabled: true, kind: Kind::Denoise { atten_db: 100.0, post_filter: 0.0 } },
                BlockCfg { id: 3, enabled: true, kind: Kind::Limiter { ceiling_db: -1.5, release_ms: 50.0 } },
            ],
        }
    }

    pub fn next_id(&self) -> u32 {
        self.blocks.iter().map(|b| b.id).max().unwrap_or(0) + 1
    }
}

pub fn db(g: f64) -> f32 {
    10f64.powf(g / 20.0) as f32
}

pub struct Lim {
    env: f32,
    rel: f32,
    ceil: f32,
}

impl Lim {
    pub fn new(ceiling_db: f64, release_ms: f64) -> Lim {
        let mut l = Lim { env: 0.0, rel: 0.0, ceil: 1.0 };
        l.set(ceiling_db, release_ms);
        l
    }
    pub fn set(&mut self, ceiling_db: f64, release_ms: f64) {
        self.ceil = db(ceiling_db.min(0.0));
        self.rel = (-1.0 / (release_ms.max(1.0) / 1000.0 * SR as f64)).exp() as f32;
    }
    pub fn process(&mut self, x: &mut [f32]) -> bool {
        let mut hit = false;
        for s in x.iter_mut() {
            let a = s.abs();
            self.env = if a > self.env { a } else { self.rel * self.env + (1.0 - self.rel) * a };
            if self.env > self.ceil {
                *s *= self.ceil / self.env;
                hit = true;
            }
        }
        hit
    }
}

pub struct Loud {
    ls: Biquad,
    hs: Biquad,
    c: LoudCoefs,
}

impl Loud {
    pub fn new(c: LoudCoefs) -> Loud {
        Loud { ls: Biquad::new(c.ls), hs: Biquad::new(c.hs), c }
    }
    pub fn set(&mut self, c: LoudCoefs) {
        self.ls.c = c.ls;
        self.hs.c = c.hs;
        self.c = c;
    }
    pub fn process(&mut self, x: &mut [f32]) {
        if self.c.neutral {
            return;
        }
        let att = self.c.att;
        for v in x.iter_mut() {
            let y = self.ls.tick(*v as f64) * att;
            *v = self.hs.tick(y) as f32;
        }
        self.ls.flush_denormals();
        self.hs.flush_denormals();
    }
}

pub struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
}

impl DelayLine {
    pub fn new(samples: usize) -> DelayLine {
        DelayLine { buf: vec![0.0; samples], pos: 0 }
    }
    pub fn process(&mut self, x: &mut [f32]) {
        let n = self.buf.len();
        if n == 0 {
            return;
        }
        for v in x.iter_mut() {
            let o = self.buf[self.pos];
            self.buf[self.pos] = *v;
            *v = o;
            self.pos = (self.pos + 1) % n;
        }
    }
}

pub enum Fx {
    Aec,
    Denoise,
    Gain(f32),
    Bq(Biquad),
    Conv(Box<Conv>),
    Loud(Loud),
    Delay(DelayLine),
    Lim(Lim),
    Comp(Comp),
    DeEss(DeEss),
    Vst(Arc<VstPlugin>, VstIo),
    Off,
}

pub struct Slot {
    pub id: u32,
    pub enabled: bool,
    pub fx: Fx,
    /// Energia na entrada e na saída desde a última leitura do medidor.
    pub e: (f64, f64),
}

pub struct Chain {
    pub slots: Vec<Slot>,
}

/// Contexto de construção: plugins VST e kernels já carregados (reutilizados ao reordenar).
pub struct Builder<'a> {
    pub vst: &'a mut BTreeMap<u32, Arc<VstPlugin>>,
    /// Volume mestre do Windows em dB (Loudness correction).
    pub vol_db: f64,
    pub base: &'a std::path::Path,
}

impl Builder<'_> {
    /// Retorna o efeito e, se falhou, a mensagem de erro (o bloco vira passagem direta).
    pub fn build(&mut self, b: &BlockCfg) -> (Fx, Option<String>) {
        match &b.kind {
            Kind::Aec { .. } => (Fx::Aec, None),
            Kind::Denoise { .. } => (Fx::Denoise, None),
            Kind::Preamp { gain_db } => (Fx::Gain(db(*gain_db)), None),
            Kind::Filter { p } => (Fx::Bq(Biquad::new(Coefs::new(p, SR as f64))), None),
            Kind::GraphicEq { nodes, .. } => {
                if nodes.iter().all(|(_, g)| g.abs() < 1e-6) {
                    (Fx::Conv(Box::new(Conv::new(&[1.0]))), None)
                } else {
                    (Fx::Conv(Box::new(Conv::new(&super::geq::design(nodes, SR as f64)))), None)
                }
            }
            Kind::Loudness { ref_level, ref_offset, attenuation } => {
                (Fx::Loud(Loud::new(super::loudness::coefs(*ref_level, *ref_offset, *attenuation, self.vol_db, SR as f64))), None)
            }
            Kind::Convolution { file } => {
                if file.is_empty() {
                    return (Fx::Off, Some("escolha um arquivo .wav".into()));
                }
                let p = self.base.join(file);
                match load_ir(&p) {
                    Ok(ir) => (Fx::Conv(Box::new(Conv::new(&ir))), None),
                    Err(e) => (Fx::Off, Some(e)),
                }
            }
            Kind::Delay { ms } => (Fx::Delay(DelayLine::new((ms.clamp(0.0, 10_000.0) * SR as f64 / 1000.0) as usize)), None),
            Kind::Limiter { ceiling_db, release_ms } => (Fx::Lim(Lim::new(*ceiling_db, *release_ms)), None),
            Kind::Compressor { p } => (Fx::Comp(Comp::new(*p)), None),
            Kind::DeEsser { p } => (Fx::DeEss(DeEss::new(*p)), None),
            Kind::Vst { library, chunk, params } => {
                let p = match self.vst.get(&b.id) {
                    Some(p) => p.clone(),
                    None => match VstPlugin::load(&self.base.join(library)) {
                        Ok(p) => {
                            p.set_state(chunk.as_deref(), params);
                            self.vst.insert(b.id, p.clone());
                            p
                        }
                        Err(e) => return (Fx::Off, Some(e)),
                    },
                };
                let io = VstIo::new(&p);
                (Fx::Vst(p, io), None)
            }
        }
    }
}

/// Comandos da interface para a thread de áudio (sem alocar no lado do áudio).
pub enum Cmd {
    SetChain(Box<Chain>),
    Enable(u32, bool),
    Coefs(u32, Coefs),
    Gain(u32, f32),
    Lim(u32, f64, f64),
    Comp(u32, CompParams),
    DeEss(u32, DeEssParams),
    Loud(u32, LoudCoefs),
    Replace(u32, Box<Fx>),
    Atten(f32),
    AecTuning(String),
}

/// Volta da thread de áudio para ser liberado fora dela.
pub enum Garbage {
    Chain(Box<Chain>),
    Fx(Box<Fx>),
    Str(String),
    Dfn(crate::dsp::Moved<crate::dsp::Dfn>),
}

impl Chain {
    pub fn slot(&mut self, id: u32) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|s| s.id == id)
    }
}

pub fn frame_ok(x: &[f32]) -> bool {
    x.len() == FRAME
}
