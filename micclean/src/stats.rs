use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

#[derive(Default)]
pub struct Stats {
    pub frames: AtomicU64,
    pub out_underruns: AtomicU64,
    /// O render do CABLE ficou vazio antes de escrevermos (o Windows tocou silêncio).
    pub out_starves: AtomicU64,
    pub low_latency: AtomicBool,
    /// Período do render da saída e da escuta em frames (0 = desconectado).
    pub out_period: AtomicU32,
    pub mon_period: AtomicU32,
    pub out_overruns: AtomicU64,
    pub ref_underruns: AtomicU64,
    pub ref_overruns: AtomicU64,
    pub mic_glitches: AtomicU64,
    pub nan_frames: AtomicU64,
    pub limited_frames: AtomicU64,
    pub proc_ns_sum: AtomicU64,
    pub proc_ns_max: AtomicU64,
    pub aec_ns_max: AtomicU64,
    pub dfn_ns_max: AtomicU64,
    pub out_ppm: AtomicF64,
    pub ref_ppm: AtomicF64,
    pub out_fill_ms: AtomicF64,
    pub ref_fill_ms: AtomicF64,
    pub aec_delay_ms: AtomicF64,
    pub aec_erle_db: AtomicF64,
    pub mic_connects: AtomicU64,
    pub ref_connects: AtomicU64,
    pub out_connects: AtomicU64,
    pub atten_db: AtomicF64,
    /// Picos desde a última leitura da interface (bits de f32).
    pub in_peak: AtomicU32,
    pub out_peak: AtomicU32,
    pub mute: AtomicBool,
    pub bypass: AtomicBool,
    pub monitor: AtomicBool,
    pub monitor_gain: AtomicF64,
    pub mic_ok: AtomicBool,
    pub out_ok: AtomicBool,
    pub ref_ok: AtomicBool,
    pub mon_ok: AtomicBool,
    pub proc_ms_avg: AtomicF64,
    pub cpu_pct: AtomicF64,
    /// Por bloco: id nos 32 bits altos, dB (f32) nos baixos. NaN = sem sinal ou desligado.
    pub blocks: [AtomicU64; MAX_BLOCKS],
    pub nblocks: AtomicU32,
    /// Média quadrática do último quadro do mic, antes de qualquer bloco.
    pub in_ms: AtomicF64,
}

pub const MAX_BLOCKS: usize = 32;

/// (id, quanto o bloco muda o nível em dB).
pub fn block_meters(st: &Stats) -> Vec<(u32, f32)> {
    let n = (st.nblocks.load(Relaxed) as usize).min(MAX_BLOCKS);
    st.blocks[..n].iter().map(|a| {
        let v = a.load(Relaxed);
        ((v >> 32) as u32, f32::from_bits(v as u32))
    }).collect()
}

pub fn peak_max(a: &AtomicU32, x: &[f32]) {
    let p = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    a.fetch_max(p.to_bits(), Relaxed);
}

pub fn take_peak(a: &AtomicU32) -> f32 {
    f32::from_bits(a.swap(0, Relaxed))
}

#[derive(Default)]
pub struct AtomicF64(AtomicU64);
impl AtomicF64 {
    pub fn set(&self, v: f64) {
        self.0.store(v.to_bits(), Relaxed)
    }
    pub fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Relaxed))
    }
}

pub fn inc(a: &AtomicU64) {
    a.fetch_add(1, Relaxed);
}
pub fn get(a: &AtomicU64) -> u64 {
    a.load(Relaxed)
}
