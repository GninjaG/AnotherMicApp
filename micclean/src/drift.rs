use crate::{FRAME, SR};
use rtrb::{Consumer, Producer};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Adjustable, Async, FixedAsync, PolynomialDegree, Resampler};

const MAX_DEV: f64 = 0.002;

/// Controle PI do nível do buffer -> razão relativa do resampler.
pub struct RateCtl {
    target: f64,
    kp: f64,
    ki: f64,
    integ: f64,
    ema: Option<f64>,
    alpha: f64,
    pub ratio: f64,
    ratio_avg: Option<f64>,
}

impl RateCtl {
    pub fn new(target_frames: f64) -> Self {
        Self { target: target_frames, kp: 0.1, ki: 0.0025, integ: 0.0, ema: None, alpha: 0.01, ratio: 1.0, ratio_avg: None }
    }

    /// `fill` em frames, chamado a cada FRAME.
    pub fn update(&mut self, fill: f64) -> f64 {
        let ema = match self.ema {
            Some(e) => e + self.alpha * (fill - e),
            None => fill,
        };
        self.ema = Some(ema);
        let e = (ema - self.target) / SR as f64;
        let dt = FRAME as f64 / SR as f64;
        if e.abs() < 0.005 {
            self.integ = (self.integ + e * dt).clamp(-MAX_DEV / self.ki, MAX_DEV / self.ki);
        }
        let u = (self.kp * e + self.ki * self.integ).clamp(-MAX_DEV, MAX_DEV);
        self.ratio = 1.0 - u;
        self.ratio_avg = Some(match self.ratio_avg {
            Some(a) => a + (1.0 / 6000.0) * (self.ratio - a),
            None => self.ratio,
        });
        self.ratio
    }

    pub fn set_target(&mut self, frames: f64) {
        self.target = frames;
        self.integ = 0.0;
    }

    pub fn reset_level(&mut self) {
        self.ema = None;
    }

    /// Deriva estimada em ppm (média lenta da razão aplicada).
    pub fn drift_ppm(&self) -> f64 {
        (self.ratio_avg.unwrap_or(1.0) - 1.0) * 1e6
    }

    pub fn smoothed_fill(&self) -> f64 {
        self.ema.unwrap_or(0.0)
    }
}

fn make_resampler(fixed: FixedAsync) -> Async<f32> {
    Async::<f32>::new_poly(1.0, 1.01, PolynomialDegree::Septic, FRAME, 1, fixed).expect("resampler")
}

#[derive(Default, Debug, Clone, Copy)]
pub struct PathEvent {
    pub underrun: bool,
    pub overrun: bool,
    /// Descartou o excesso acima do alvo.
    pub dropped: bool,
}

/// Referência: consome do ring (relógio das caixas), entrega FRAME no relógio do mic.
pub struct RefPath {
    rs: Async<f32>,
    pub ctl: RateCtl,
    inbuf: Vec<f32>,
    target: usize,
    max: usize,
    primed: bool,
}

impl RefPath {
    pub fn new(target_ms: f64) -> Self {
        let rs = make_resampler(FixedAsync::Output);
        let target = (target_ms * SR as f64 / 1000.0) as usize;
        let inbuf = vec![0.0; rs.input_frames_max()];
        Self { rs, ctl: RateCtl::new(target as f64), inbuf, target, max: target * 4 + 2 * FRAME, primed: false }
    }

    pub fn latency_frames(&self) -> usize {
        self.target + self.rs.output_delay()
    }

    pub fn pull(&mut self, cons: &mut Consumer<f32>, out: &mut [f32]) -> PathEvent {
        let mut ev = PathEvent::default();
        let fill = cons.slots();
        if fill > self.max {
            if let Ok(c) = cons.read_chunk(fill - self.target) {
                c.commit_all();
            }
            self.ctl.reset_level();
            ev.overrun = true;
        }
        if !self.primed {
            if cons.slots() < self.target {
                out[..FRAME].fill(0.0);
                return ev;
            }
            self.primed = true;
            self.ctl.reset_level();
        }
        let r = self.ctl.update(cons.slots() as f64);
        let _ = self.rs.set_resample_ratio_relative(r, true);
        let need = self.rs.input_frames_next();
        let n = need.min(cons.slots());
        if let Ok(c) = cons.read_chunk(n) {
            let (a, b) = c.as_slices();
            self.inbuf[..a.len()].copy_from_slice(a);
            self.inbuf[a.len()..a.len() + b.len()].copy_from_slice(b);
            c.commit_all();
        }
        if n < need {
            self.inbuf[n..need].fill(0.0);
            ev.underrun = true;
            self.primed = false;
        }
        let inp = InterleavedSlice::new(&self.inbuf[..need], 1, need).unwrap();
        let mut o = InterleavedSlice::new_mut(&mut out[..FRAME], 1, FRAME).unwrap();
        let _ = self.rs.process_into_buffer(&inp, &mut o, None);
        ev
    }
}

/// Saída: recebe FRAME no relógio do mic, escreve no ring consumido pelo relógio do CABLE.
pub struct OutPath {
    rs: Async<f32>,
    pub ctl: RateCtl,
    outbuf: Vec<f32>,
    pub target: usize,
    cap: usize,
    max: usize,
    low: usize,
    nlow: usize,
    skip: usize,
}

/// Pushes (de 10 ms) para medir o menor nível do buffer.
const LOW_WIN: usize = 50;
/// Acima do alvo por mais que 1 frame + esta folga em toda a janela: descarta o excesso em vez de esperar o PI.
const SLACK: usize = SR / 500;
const XFADE: usize = 32;

impl OutPath {
    pub fn new(target_ms: f64, ring_capacity: usize) -> Self {
        let rs = make_resampler(FixedAsync::Input);
        let target = (target_ms * SR as f64 / 1000.0) as usize;
        let outbuf = vec![0.0; rs.output_frames_max()];
        Self { rs, ctl: RateCtl::new(target as f64), outbuf, target, cap: ring_capacity, max: target * 2 + 4 * FRAME, low: usize::MAX, nlow: 0, skip: 0 }
    }

    pub fn latency_frames(&self) -> usize {
        self.target + self.rs.output_delay()
    }

    pub fn set_target(&mut self, target_ms: f64) {
        self.target = (target_ms * SR as f64 / 1000.0) as usize;
        self.max = self.target * 2 + 4 * FRAME;
        self.ctl.set_target(self.target as f64);
        (self.low, self.nlow) = (usize::MAX, 0);
    }

    pub fn push(&mut self, input: &[f32], prod: &mut Producer<f32>) -> PathEvent {
        let mut ev = PathEvent::default();
        let fill = self.cap - prod.slots();
        // rajada do mic: descarta o frame até a latência voltar perto do alvo
        if fill > self.max {
            self.ctl.reset_level();
            ev.overrun = true;
            return ev;
        }
        // o nível logo antes do push é o ponto mais baixo do ciclo
        self.low = self.low.min(fill);
        self.nlow += 1;
        if self.nlow >= LOW_WIN {
            if self.low > self.target + FRAME + SLACK {
                self.skip = self.low - (self.target + SLACK);
                self.ctl.reset_level();
            }
            self.low = usize::MAX;
            self.nlow = 0;
        }
        let r = self.ctl.update(fill as f64);
        let _ = self.rs.set_resample_ratio_relative(r, true);
        let inp = InterleavedSlice::new(&input[..FRAME], 1, FRAME).unwrap();
        let olen = self.outbuf.len();
        let mut o = InterleavedSlice::new_mut(&mut self.outbuf[..], 1, olen).unwrap();
        let n = match self.rs.process_into_buffer(&inp, &mut o, None) {
            Ok((_, n)) => n,
            Err(_) => 0,
        };
        let d = self.skip.min(n.saturating_sub(2 * XFADE));
        if d > 0 {
            for i in 0..XFADE {
                let g = (i as f32 + 0.5) / XFADE as f32;
                self.outbuf[d + i] = self.outbuf[i] * (1.0 - g) + self.outbuf[d + i] * g;
            }
            self.skip -= d;
            ev.dropped = true;
        }
        let src = &self.outbuf[d..n];
        let space = prod.slots();
        let w = if src.len() > space {
            ev.overrun = true;
            space
        } else {
            src.len()
        };
        if let Ok(mut c) = prod.write_chunk(w) {
            let (a, b) = c.as_mut_slices();
            let la = a.len();
            a.copy_from_slice(&src[..la]);
            b.copy_from_slice(&src[la..la + b.len()]);
            c.commit_all();
        }
        ev
    }
}

/// Lado do dispositivo de saída: lê `out.len()` amostras, com preenchimento inicial.
pub struct OutReader {
    primed: bool,
    target: usize,
}

impl OutReader {
    pub fn new(target: usize) -> Self {
        Self { primed: false, target }
    }

    pub fn set_target(&mut self, target: usize) {
        self.target = target;
    }

    pub fn read(&mut self, cons: &mut Consumer<f32>, out: &mut [f32]) -> bool {
        let need = out.len();
        if !self.primed {
            if cons.slots() >= self.target.max(need) {
                self.primed = true;
            } else {
                out.fill(0.0);
                return false;
            }
        }
        let n = need.min(cons.slots());
        if let Ok(c) = cons.read_chunk(n) {
            let (a, b) = c.as_slices();
            out[..a.len()].copy_from_slice(a);
            out[a.len()..a.len() + b.len()].copy_from_slice(b);
            c.commit_all();
        }
        if n < need {
            out[n..].fill(0.0);
            self.primed = false;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::RingBuffer;

    #[test]
    fn poly_septic_low_latency_and_transparent() {
        let mut outp = OutPath::new(10.0, SR / 2);
        let refp = RefPath::new(25.0);
        assert!(outp.latency_frames() - outp.target <= 8, "out delay: {}", outp.latency_frames() - outp.target);
        assert!(refp.latency_frames() - refp.target <= 8, "ref delay: {}", refp.latency_frames() - refp.target);

        let (mut prod, mut cons) = RingBuffer::<f32>::new(SR / 2);
        let mut reader = OutReader::new(outp.target);
        let mut out = [0.0f32; FRAME];
        for freq in [1000.0f64, 10_000.0] {
            let mut sum_in = 0.0f64;
            let mut sum_out = 0.0f64;
            let mut phase = 0.0f64;
            let mut inp = [0.0f32; FRAME];
            for k in 0..200 {
                for s in inp.iter_mut() {
                    *s = 0.25 * (2.0 * std::f64::consts::PI * phase).sin() as f32;
                    phase += freq / SR as f64;
                }
                let _ = outp.rs.set_resample_ratio_relative(1.0002, true);
                outp.push(&inp, &mut prod);
                reader.read(&mut cons, &mut out);
                if k >= 50 {
                    sum_in += inp.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>();
                    sum_out += out.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>();
                }
            }
            let diff_db = 10.0 * (sum_out / sum_in.max(1e-20)).log10();
            assert!(diff_db.abs() < 0.1, "{freq} Hz gain error: {diff_db:.3} dB");
        }
        // Silencia warning de variável refp usada acima
        let _ = refp.ctl.drift_ppm();
    }
}
