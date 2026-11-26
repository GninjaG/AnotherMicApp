use crate::audio::{self, Res, Wake};
use crate::config::{Config, Devices, out_target_ms, out_target_for};
use crate::drift::{OutPath, OutReader, RefPath};
use crate::dsp::Pipeline;
use crate::fx::chain::{Chain, Cmd, Garbage, Preset};
use crate::stats::{Stats, get, inc};
use crate::{FRAME, SR, log};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::{Acquire, Relaxed, Release}};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use wasapi::Direction;

const RING: usize = SR / 2;
pub const SPEC_RING: usize = 16384;

/// Dispositivos atuais; `gen` muda quando a interface troca algum (as threads reconectam).
pub struct DevState {
    pub d: Mutex<Devices>,
    pub generation: AtomicU64,
}

impl DevState {
    fn snap(&self) -> (Devices, u64) {
        (self.d.lock().unwrap().clone(), self.generation.load(Acquire))
    }
    fn changed(&self, g: u64) -> bool {
        self.generation.load(Acquire) != g
    }
}

pub struct Engine {
    stop: Arc<AtomicBool>,
    wake: Arc<Wake>,
    handles: Mutex<Vec<JoinHandle<()>>>,
    pub stats: Arc<Stats>,
    pub devs: Arc<DevState>,
    cmd: Mutex<Producer<Cmd>>,
    bin: Mutex<Consumer<Garbage>>,
    pub spec: Mutex<Consumer<f32>>,
}

impl Engine {
    pub fn start(cfg: &Config, preset: &Preset, chain: Box<Chain>) -> Res<Engine> {
        let stats = Arc::new(Stats::default());
        stats.monitor_gain.set(10f64.powf(cfg.app.monitor_gain_db / 20.0));
        stats.low_latency.store(cfg.app.low_latency, Relaxed);
        stats.active_ll.store(cfg.app.low_latency, Release);
        let ll = cfg.app.low_latency;
        let out_ms = cfg.buffers.out_ms;
        let devs = Arc::new(DevState { d: Mutex::new(cfg.devices.clone()), generation: AtomicU64::new(0) });
        let stop = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(Wake::new());
        let (out_prod, out_cons) = RingBuffer::<f32>::new(RING);
        let (ref_prod, ref_cons) = RingBuffer::<f32>::new(RING);
        let (mon_prod, mon_cons) = RingBuffer::<f32>::new(RING);
        let (spec_prod, spec_cons) = RingBuffer::<f32>::new(SPEC_RING);
        let (cmd_prod, cmd_cons) = RingBuffer::<Cmd>::new(256);
        let (bin_prod, bin_cons) = RingBuffer::<Garbage>::new(256);
        log!("cadeia '{}': {} blocos, out_ms={} ref_ms={}", preset.name, preset.blocks.len(), cfg.buffers.out_ms, cfg.buffers.ref_ms);
        let mut handles = Vec::new();
        // DfTract não é Send: o pipeline é criado dentro da thread do mic.
        let (tx, rx) = std::sync::mpsc::channel::<Res<()>>();
        let (c, s, w, st, dv, pr) = (cfg.clone(), stop.clone(), wake.clone(), stats.clone(), devs.clone(), preset.clone());
        handles.push(spawn("mic-dsp", move || {
            let pipe = match Pipeline::with_model(&pr, chain, ll) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let aec_active = pipe.has_aec();
            st.aec_active.store(aec_active, Release);
            let dsp = Dsp {
                pipe,
                refp: RefPath::new(c.buffers.ref_ms),
                outp: OutPath::new(out_target_ms(out_ms, ll), RING),
                monp: OutPath::new(out_target_ms(out_ms, ll), RING),
                out_ms,
                targets: (out_target_ms(out_ms, ll), out_target_ms(out_ms, ll)),
                loading: None,
                aec_active,
                wake: w.clone(),
                ref_cons,
                out_prod,
                mon_prod,
                spec_prod,
                cmd_cons,
                bin_prod,
                frame: [0.0; FRAME],
                refb: [0.0; FRAME],
                fill: 0,
                stats: st.clone(),
            };
            log!(
                "latência de buffer: saída {:.1} ms, referência {:.1} ms",
                dsp.outp.latency_frames() as f64 * 1000.0 / SR as f64,
                dsp.refp.latency_frames() as f64 * 1000.0 / SR as f64
            );
            let _ = tx.send(Ok(()));
            mic_thread(dv, dsp, st, s)
        }));
        let init = rx.recv().unwrap_or_else(|_| Err("thread do mic morreu".into()));
        if let Err(e) = init {
            stop.store(true, Release);
            wake.notify_all();
            return Err(format!("pipeline: {e}"));
        }
        let (dv, s, w, st) = (devs.clone(), stop.clone(), wake.clone(), stats.clone());
        handles.push(spawn("loopback", move || ref_thread(dv, ref_prod, st, s, w)));
        let (dv, s, w, st) = (devs.clone(), stop.clone(), wake.clone(), stats.clone());
        handles.push(spawn("output", move || out_thread(dv, out_cons, out_ms, st, s, w, false)));
        let (dv, s, w, st) = (devs.clone(), stop.clone(), wake.clone(), stats.clone());
        handles.push(spawn("monitor", move || out_thread(dv, mon_cons, out_ms, st, s, w, true)));
        let (s, w) = (stop.clone(), wake.clone());
        handles.push(spawn("hotplug", move || audio::watch_devices(&s, &w)));
        let (s, w, st, iv) = (stop.clone(), wake.clone(), stats.clone(), cfg.app.stats_interval_s.max(1));
        handles.push(spawn("stats", move || stats_thread(st, s, w, iv)));
        Ok(Engine {
            stop,
            wake,
            handles: Mutex::new(handles),
            stats,
            devs,
            cmd: Mutex::new(cmd_prod),
            bin: Mutex::new(bin_cons),
            spec: Mutex::new(spec_cons),
        })
    }

    /// Envia um comando para a thread de áudio. Falha só se a fila estiver cheia.
    pub fn send(&self, c: Cmd) -> bool {
        self.cmd.lock().unwrap().push(c).is_ok()
    }

    /// Libera objetos que a thread de áudio descartou.
    pub fn collect(&self) {
        let mut b = self.bin.lock().unwrap();
        while b.pop().is_ok() {}
    }

    pub fn set_devices(&self, d: Devices) {
        *self.devs.d.lock().unwrap() = d;
        self.devs.generation.fetch_add(1, Release);
        self.wake.notify_all();
    }

    pub fn set_monitor(&self, on: bool) {
        self.stats.monitor.store(on, Release);
        self.wake.notify_all();
    }

    pub fn stop(&self) {
        self.stop.store(true, Release);
        self.wake.notify_all();
        let mut hs = self.handles.lock().unwrap();
        if hs.is_empty() {
            return;
        }
        for h in hs.drain(..) {
            let _ = h.join();
        }
        log!("engine parado");
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    std::thread::Builder::new().name(name.into()).spawn(f).expect("thread")
}

/// Espera para tentar de novo; acorda antes se um dispositivo entrar ou sair.
fn wait_retry(stop: &AtomicBool, devs: &DevState, g: u64, d: Duration, wake: &Wake) {
    let h = audio::hotplug();
    wake.wait_timeout_while(d, || !stop.load(Acquire) && !devs.changed(g) && audio::hotplug() == h);
}

/// Depois de um aviso do Windows, confere se `pattern` ainda aponta para o mesmo aparelho.
fn moved(dir: Direction, pattern: &str, id: &str, h: &mut u64) -> bool {
    let now = audio::hotplug();
    if now == *h {
        return false;
    }
    *h = now;
    audio::find(dir, pattern).and_then(|d| d.get_id().map_err(|e| e.to_string())).map(|x| x != id).unwrap_or(true)
}

fn sleep_unless(stop: &AtomicBool, d: Duration, wake: &Wake) {
    wake.wait_timeout_while(d, || !stop.load(Acquire));
}

struct Dsp {
    pipe: Pipeline,
    refp: RefPath,
    outp: OutPath,
    monp: OutPath,
    ref_cons: Consumer<f32>,
    out_prod: Producer<f32>,
    mon_prod: Producer<f32>,
    spec_prod: Producer<f32>,
    cmd_cons: Consumer<Cmd>,
    bin_prod: Producer<Garbage>,
    frame: [f32; FRAME],
    refb: [f32; FRAME],
    fill: usize,
    out_ms: f64,
    targets: (f64, f64),
    loading: Option<(bool, std::sync::mpsc::Receiver<Result<crate::dsp::Moved<crate::dsp::Dfn>, String>>)>,
    aec_active: bool,
    wake: Arc<Wake>,
    stats: Arc<Stats>,
}

impl Dsp {
    fn feed(&mut self, mut s: &[f32]) {
        while !s.is_empty() {
            let n = s.len().min(FRAME - self.fill);
            self.frame[self.fill..self.fill + n].copy_from_slice(&s[..n]);
            self.fill += n;
            s = &s[n..];
            if self.fill == FRAME {
                self.fill = 0;
                self.process();
            }
        }
    }

    fn process(&mut self) {
        let st = &*self.stats;
        while let Ok(c) = self.cmd_cons.pop() {
            self.pipe.apply(c, &mut self.bin_prod);
        }
        let has_aec = self.pipe.has_aec();
        if has_aec != self.aec_active {
            self.aec_active = has_aec;
            st.aec_active.store(has_aec, Release);
            self.wake.notify_all();
        }
        let ll = st.low_latency.load(Relaxed);
        if let Some((want, rx)) = &self.loading {
            match rx.try_recv() {
                Ok(Ok(d)) if *want == ll => {
                    let old = self.pipe.swap_model(d, ll);
                    if let Err(rtrb::PushError::Full(g)) = self.bin_prod.push(Garbage::Dfn(old)) {
                        std::mem::forget(g);
                    }
                    st.active_ll.store(ll, Release);
                    self.loading = None;
                }
                Ok(Ok(d)) => {
                    if let Err(rtrb::PushError::Full(g)) = self.bin_prod.push(Garbage::Dfn(d)) {
                        std::mem::forget(g);
                    }
                    self.loading = None;
                }
                Ok(Err(_)) => {
                    st.low_latency.store(self.pipe.ll, Relaxed);
                    self.loading = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => self.loading = None,
            }
        } else if ll != self.pipe.ll {
            self.loading = Some((ll, self.pipe.load_model(ll)));
        }
        let ll = self.pipe.ll;
        let t = (out_target_for(self.out_ms, ll, st.out_period.load(Relaxed)), out_target_for(self.out_ms, ll, st.mon_period.load(Relaxed)));
        if t.0 != self.targets.0 {
            self.outp.set_target(t.0);
        }
        if t.1 != self.targets.1 {
            self.monp.set_target(t.1);
        }
        self.targets = t;
        self.pipe.mute = st.mute.load(Relaxed);
        self.pipe.bypass = st.bypass.load(Relaxed);
        let t0 = Instant::now();
        if self.aec_active {
            let ev = self.refp.pull(&mut self.ref_cons, &mut self.refb);
            if ev.underrun {
                inc(&st.ref_underruns);
            }
            if ev.overrun {
                inc(&st.ref_overruns);
            }
        } else {
            let n = self.ref_cons.slots();
            if n > 0 {
                if let Ok(ch) = self.ref_cons.read_chunk(n) {
                    ch.commit_all();
                }
            }
            self.refb.fill(0.0);
        }
        let (mut in_pk, mut in_sq) = (0.0f32, 0.0f64);
        for &v in &self.frame {
            in_pk = in_pk.max(v.abs());
            let vf = v as f64;
            in_sq += vf * vf;
        }
        st.in_peak.fetch_max(in_pk.to_bits(), Relaxed);
        st.in_ms.set(in_sq / FRAME as f64);
        let t = self.pipe.process(&mut self.frame, &self.refb);
        let out_pk = self.frame.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        st.out_peak.fetch_max(out_pk.to_bits(), Relaxed);
        let ev = self.outp.push(&self.frame, &mut self.out_prod);
        if ev.overrun || ev.dropped {
            inc(&st.out_overruns);
        }
        if st.monitor.load(Relaxed) {
            self.monp.push(&self.frame, &mut self.mon_prod);
        }
        if self.spec_prod.slots() >= FRAME {
            if let Ok(mut ch) = self.spec_prod.write_chunk(FRAME) {
                let (a, b) = ch.as_mut_slices();
                let la = a.len();
                a.copy_from_slice(&self.frame[..la]);
                b.copy_from_slice(&self.frame[la..la + b.len()]);
                ch.commit_all();
            }
        }
        if t.nan {
            inc(&st.nan_frames);
        }
        if t.limited {
            inc(&st.limited_frames);
        }
        let ns = t0.elapsed().as_nanos() as u64;
        st.proc_ns_sum.fetch_add(ns, Relaxed);
        st.proc_ns_max.fetch_max(ns, Relaxed);
        st.aec_ns_max.fetch_max(t.aec_ns, Relaxed);
        st.dfn_ns_max.fetch_max(t.dfn_ns, Relaxed);
        let n = st.frames.fetch_add(1, Relaxed);
        if n % 10 == 0 {
            // ~-60 dBFS de média em 100 ms: abaixo disso não mede
            let floor = 1e-6 * (10 * FRAME) as f64;
            let mut k = 0;
            for s in self.pipe.chain.slots.iter_mut().take(crate::stats::MAX_BLOCKS) {
                let (a, b) = std::mem::take(&mut s.e);
                let v = if a > floor { (10.0 * ((b + 1e-12) / a).log10()) as f32 } else { f32::NAN };
                st.blocks[k].store(((s.id as u64) << 32) | v.to_bits() as u64, Relaxed);
                k += 1;
            }
            st.nblocks.store(k as u32, Release);
        }
        if n % 100 == 0 {
            let ms = 1000.0 / SR as f64;
            st.out_ppm.set(self.outp.ctl.drift_ppm());
            st.ref_ppm.set(self.refp.ctl.drift_ppm());
            st.out_fill_ms.set(self.outp.ctl.smoothed_fill() * ms);
            st.ref_fill_ms.set(self.refp.ctl.smoothed_fill() * ms);
            if self.aec_active {
                let (d, erle) = self.pipe.aec.metrics();
                st.aec_delay_ms.set(d.map(|x| x as f64).unwrap_or(f64::NAN));
                st.aec_erle_db.set(erle.unwrap_or(f64::NAN));
            } else {
                st.aec_delay_ms.set(f64::NAN);
                st.aec_erle_db.set(f64::NAN);
            }
        }
    }
}

fn mic_thread(devs: Arc<DevState>, mut dsp: Dsp, st: Arc<Stats>, stop: Arc<AtomicBool>) {
    audio::com_init();
    let _mm = audio::mmcss_pro_audio();
    if _mm.is_none() {
        log!("aviso: MMCSS indisponível na thread do mic");
    }
    let mut last_err = String::new();
    while !stop.load(Acquire) {
        let (d, g) = devs.snap();
        let r = (|| -> Res<()> {
            let mut h = audio::hotplug();
            let dev = audio::find(Direction::Capture, &d.mic)?;
            let id = dev.get_id().map_err(|e| e.to_string())?;
            let mut c = match audio::open_capture_fast(&dev, d.mic_raw) {
                Ok(c) => c,
                Err(e) => {
                    log!("captura: {e}, usando período padrão");
                    audio::open_capture(&dev, false, d.mic_raw, 1)?
                }
            };
            dsp.fill = 0;
            c.client.start_stream().map_err(|e| e.to_string())?;
            inc(&st.mic_connects);
            st.mic_ok.store(true, Relaxed);
            log!("mic conectado: '{}'", c.name);
            last_err.clear();
            let mut timeouts = 0;
            while !stop.load(Acquire) && !devs.changed(g) {
                if moved(Direction::Capture, &d.mic, &id, &mut h) {
                    let _ = c.client.stop_stream();
                    return Err("o microfone mudou ou saiu".into());
                }
                if c.ev.wait_for_event(200).is_err() {
                    timeouts += 1;
                    if timeouts > 10 {
                        return Err(format!("mic '{}' sem dados por 2 s", c.name));
                    }
                    continue;
                }
                timeouts = 0;
                let (_, disc) = c.drain(|s| dsp.feed(s))?;
                if disc {
                    inc(&st.mic_glitches);
                }
            }
            let _ = c.client.stop_stream();
            Ok(())
        })();
        st.mic_ok.store(false, Relaxed);
        if let Err(e) = r {
            if e != last_err {
                log!("mic: {e} (tentando de novo)");
                last_err = e;
            }
            // sem mic: a cadeia continua recebendo comandos
            let (t, h) = (Instant::now(), audio::hotplug());
            while t.elapsed() < Duration::from_secs(1) && !stop.load(Acquire) && !devs.changed(g) && audio::hotplug() == h {
                while let Ok(c) = dsp.cmd_cons.pop() {
                    dsp.pipe.apply(c, &mut dsp.bin_prod);
                }
                let has_aec = dsp.pipe.has_aec();
                if has_aec != dsp.aec_active {
                    dsp.aec_active = has_aec;
                    st.aec_active.store(has_aec, Release);
                    dsp.wake.notify_all();
                }
                // descarta a referência que ninguém vai consumir
                let n = dsp.ref_cons.slots();
                if n > 0 {
                    if let Ok(ch) = dsp.ref_cons.read_chunk(n) {
                        ch.commit_all();
                    }
                }
                dsp.wake.wait_timeout_while(Duration::from_millis(50), || !stop.load(Acquire) && !devs.changed(g) && audio::hotplug() == h);
            }
        }
    }
}

fn ref_thread(devs: Arc<DevState>, mut prod: Producer<f32>, st: Arc<Stats>, stop: Arc<AtomicBool>, wake: Arc<Wake>) {
    audio::com_init();
    let _mm = audio::mmcss_pro_audio();
    let mut last_err = String::new();
    while !stop.load(Acquire) {
        if !st.aec_active.load(Acquire) {
            wake.wait_while(|| !stop.load(Acquire) && !st.aec_active.load(Acquire));
            continue;
        }
        let (d, g) = devs.snap();
        let r = (|| -> Res<()> {
            let mut h = audio::hotplug();
            let dev = audio::find(Direction::Render, &d.speakers)?;
            let id = dev.get_id().map_err(|e| e.to_string())?;
            if let Ok(o) = audio::find(Direction::Render, &d.output) {
                if o.get_id().ok().as_deref() == Some(&id) {
                    return Err("caixas == dispositivo de saída (CABLE). Corrija a reprodução padrão".into());
                }
            }
            let mut keep = audio::open_render(&dev, 2, Some(1_000_000))?;
            let a = keep.available()?;
            keep.write_silence(a)?;
            keep.client.start_stream().map_err(|e| e.to_string())?;
            let mut c = audio::open_capture(&dev, true, false, 1)?;
            c.client.start_stream().map_err(|e| e.to_string())?;
            inc(&st.ref_connects);
            st.ref_ok.store(true, Relaxed);
            log!("referência (loopback) conectada: '{}'", c.name);
            last_err.clear();
            let mut timeouts = 0;
            let mut last_check = Instant::now();
            while !stop.load(Acquire) && !devs.changed(g) && st.aec_active.load(Acquire) {
                if moved(Direction::Render, &d.speakers, &id, &mut h) {
                    let _ = c.client.stop_stream();
                    let _ = keep.client.stop_stream();
                    return Err("o dispositivo de referência mudou ou saiu".into());
                }
                if c.ev.wait_for_event(200).is_err() {
                    timeouts += 1;
                    if timeouts > 10 {
                        let _ = c.client.stop_stream();
                        let _ = keep.client.stop_stream();
                        return Err(format!("loopback '{}' sem dados por 2 s", c.name));
                    }
                } else {
                    timeouts = 0;
                    c.drain(|s| {
                        let n = s.len().min(prod.slots());
                        if n < s.len() {
                            inc(&st.ref_overruns);
                        }
                        if let Ok(mut ch) = prod.write_chunk(n) {
                            let (a, b) = ch.as_mut_slices();
                            let la = a.len();
                            a.copy_from_slice(&s[..la]);
                            b.copy_from_slice(&s[la..la + b.len()]);
                            ch.commit_all();
                        }
                    })?;
                }
                let a = keep.available()?;
                if a >= keep.buffer_frames as usize / 2 {
                    keep.write_silence(a)?;
                }
                if !audio::hotplug_ok() && d.speakers.trim().is_empty() && last_check.elapsed() > Duration::from_secs(2) {
                    last_check = Instant::now();
                    if audio::default_id(Direction::Render).as_deref() != Some(&id) {
                        let _ = c.client.stop_stream();
                        let _ = keep.client.stop_stream();
                        return Err("reprodução padrão mudou".into());
                    }
                }
            }
            let _ = c.client.stop_stream();
            let _ = keep.client.stop_stream();
            Ok(())
        })();
        st.ref_ok.store(false, Relaxed);
        if let Err(e) = r {
            if e != last_err {
                log!("referência: {e} (tentando de novo)");
                last_err = e;
            }
            wait_retry(&stop, &devs, g, Duration::from_secs(1), &wake);
        }
    }
}

/// Saída para o CABLE ou, com `monitor`, escuta no fone (só enquanto ligada).
fn out_thread(devs: Arc<DevState>, mut cons: Consumer<f32>, out_ms: f64, st: Arc<Stats>, stop: Arc<AtomicBool>, wake: Arc<Wake>, monitor: bool) {
    let period = if monitor { &st.mon_period } else { &st.out_period };
    let target = |ll: bool| (out_target_for(out_ms, ll, period.load(Relaxed)) * SR as f64 / 1000.0) as usize;
    audio::com_init();
    let _mm = audio::mmcss_pro_audio_critical();
    let mut last_err = String::new();
    let mut tmp = vec![0.0f32; 4096];
    let tag = if monitor { "escuta" } else { "saída" };
    let ok = if monitor { &st.mon_ok } else { &st.out_ok };
    while !stop.load(Acquire) {
        if monitor && !st.monitor.load(Acquire) {
            let n = cons.slots();
            if n > 0 {
                if let Ok(ch) = cons.read_chunk(n) {
                    ch.commit_all();
                }
            }
            wake.wait_while(|| !stop.load(Acquire) && !st.monitor.load(Acquire));
            continue;
        }
        let (d, g) = devs.snap();
        let r = (|| -> Res<()> {
            let name = if monitor { &d.monitor } else { &d.output };
            let mut h = audio::hotplug();
            let dev = audio::find(Direction::Render, name)?;
            let id = dev.get_id().map_err(|e| e.to_string())?;
            let excl = if !monitor && d.out_exclusive {
                audio::open_render_exclusive(&dev, 2).map_err(|e| log!("{tag}: {e}, usando o modo compartilhado")).ok()
            } else {
                None
            };
            let exclusive = excl.is_some();
            let mut r = match excl.map(Ok).unwrap_or_else(|| audio::open_render_fast(&dev)) {
                Ok(r) => r,
                Err(e) => {
                    log!("{tag}: {e}, usando o período padrão");
                    audio::open_render(&dev, 2, None)?
                }
            };
            let bf = r.buffer_frames as usize;
            if tmp.len() < bf {
                tmp.resize(bf, 0.0);
            }
            let per = r.period as usize;
            period.store(r.period, Relaxed);
            let mut ll = st.active_ll.load(Acquire);
            let mut reader = OutReader::new(target(ll));
            // fila no dispositivo: 3 períodos se <128 (~2.7ms), 2 se <400, 1.5 no padrão de 10 ms
            let mut queue = if exclusive {
                bf
            } else if per < 128 {
                3 * per
            } else if per < 400 {
                2 * per
            } else {
                per + per / 2
            }
            .min(bf);
            let max_queue = (3 * per).min(bf);
            let mut raised = false;
            let mut warm = 0;
            r.write_silence(queue)?;
            r.client.start_stream().map_err(|e| e.to_string())?;
            if !monitor {
                inc(&st.out_connects);
            }
            ok.store(true, Relaxed);
            log!("{tag} conectada: '{}' (latência do stream {:.1} ms)", r.name, r.latency_hns() as f64 / 1e4);
            last_err.clear();
            let mut timeouts = 0;
            while !stop.load(Acquire) && !devs.changed(g) && (!monitor || st.monitor.load(Acquire)) {
                if moved(Direction::Render, name, &id, &mut h) {
                    let _ = r.client.stop_stream();
                    return Err("o dispositivo mudou ou saiu".into());
                }
                if r.ev.wait_for_event(200).is_err() {
                    timeouts += 1;
                    if timeouts > 10 {
                        return Err(format!("{tag} '{}' parou", r.name));
                    }
                    continue;
                }
                timeouts = 0;
                let now = st.active_ll.load(Acquire);
                if now != ll {
                    ll = now;
                    reader.set_target(target(ll));
                }
                let avail = r.available()?;
                if avail >= bf && !monitor && !exclusive && warm > 50 {
                    inc(&st.out_starves);
                    if queue < max_queue {
                        queue = (queue + per).min(max_queue);
                        if !raised {
                            raised = true;
                            log!("{tag}: fila aumentada para {queue} frames após atraso");
                        }
                    }
                }
                warm += 1;
                let n = queue.saturating_sub(bf - avail).min(tmp.len());
                if n == 0 {
                    continue;
                }
                if reader.read(&mut cons, &mut tmp[..n]) && !monitor {
                    inc(&st.out_underruns);
                }
                if monitor {
                    let gn = st.monitor_gain.get() as f32;
                    tmp[..n].iter_mut().for_each(|v| *v = (*v * gn).clamp(-1.0, 1.0));
                }
                r.write_mono(&tmp[..n])?;
            }
            let _ = r.client.stop_stream();
            Ok(())
        })();
        ok.store(false, Relaxed);
        period.store(0, Relaxed);
        if let Err(e) = r {
            if e != last_err {
                log!("{tag}: {e} (tentando de novo)");
                last_err = e;
            }
            wait_retry(&stop, &devs, g, Duration::from_secs(2), &wake);
        }
    }
}

fn stats_thread(st: Arc<Stats>, stop: Arc<AtomicBool>, wake: Arc<Wake>, interval: u64) {
    let mut last_frames = 0;
    let mut last_sum = 0;
    let mut prev = [0u64; 6];
    let mut tick = Instant::now();
    let mut lf = 0;
    let mut ls = 0;
    let mut last_ll = st.active_ll.load(Acquire);
    while !stop.load(Acquire) {
        // métricas rápidas para a janela, a cada 1 s
        sleep_unless(&stop, Duration::from_secs(1), &wake);
        let ll = st.active_ll.load(Acquire);
        if ll != last_ll {
            last_ll = ll;
            log!("modo de baixa latência: {}", if ll { "ligado" } else { "desligado" });
        }
        let (f, s) = (get(&st.frames), get(&st.proc_ns_sum));
        if f > lf {
            let avg = s.saturating_sub(ls) as f64 / (f - lf) as f64 / 1e6;
            st.proc_ms_avg.set(avg);
            st.cpu_pct.set(avg / 10.0 * 100.0);
        }
        (lf, ls) = (f, s);
        if tick.elapsed() < Duration::from_secs(interval) {
            continue;
        }
        tick = Instant::now();
        let frames = get(&st.frames);
        let sum = get(&st.proc_ns_sum);
        let df = frames.saturating_sub(last_frames);
        let avg = if df > 0 { sum.saturating_sub(last_sum) as f64 / df as f64 / 1e6 } else { 0.0 };
        last_frames = frames;
        last_sum = sum;
        let cur = [
            get(&st.out_underruns),
            get(&st.out_overruns),
            get(&st.ref_underruns),
            get(&st.ref_overruns),
            get(&st.mic_glitches),
            get(&st.out_starves),
        ];
        let d: [u64; 6] = std::array::from_fn(|i| cur[i].saturating_sub(prev[i]));
        prev = cur;
        log!(
            "stats: frames={} proc_med={:.3}ms proc_max={:.3}ms (aec_max={:.3} dfn_max={:.3}) | out_underrun=+{}({}) out_overrun=+{}({}) ref_underrun=+{}({}) ref_overrun=+{}({}) mic_glitch=+{}({}) out_starve=+{}({}) nan={} limit={} | deriva out={:+.1}ppm ref={:+.1}ppm | buf out={:.1}ms ref={:.1}ms | aec atraso={:.0}ms erle={:.1}dB",
            frames,
            avg,
            st.proc_ns_max.swap(0, Relaxed) as f64 / 1e6,
            st.aec_ns_max.swap(0, Relaxed) as f64 / 1e6,
            st.dfn_ns_max.swap(0, Relaxed) as f64 / 1e6,
            d[0], cur[0], d[1], cur[1], d[2], cur[2], d[3], cur[3], d[4], cur[4], d[5], cur[5],
            get(&st.nan_frames),
            get(&st.limited_frames),
            st.out_ppm.get(),
            st.ref_ppm.get(),
            st.out_fill_ms.get(),
            st.ref_fill_ms.get(),
            st.aec_delay_ms.get(),
            st.aec_erle_db.get()
        );
    }
}
