use micclean::audio::{self, Res};
use micclean::config::Config;
use micclean::{SR, wav};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use wasapi::Direction;

fn testdata(name: &str) -> Vec<f32> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata").join(name);
    wav::read_mono(&p).expect("testdata").0
}

fn workdir() -> PathBuf {
    let d = std::env::current_exe().unwrap().parent().unwrap().join("pc_tests");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rms_db(x: &[f32]) -> f64 {
    let e: f64 = x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len().max(1) as f64;
    10.0 * (e + 1e-20).log10()
}

fn dc_block(x: &[f32]) -> Vec<f32> {
    let mut v = x.to_vec();
    let mut d = micclean::dsp::DcBlock::default();
    d.process(&mut v);
    let mut d = micclean::dsp::DcBlock::default();
    d.process(&mut v);
    v
}

struct Rec {
    data: Vec<f32>,
    t0: u64,
    last: (usize, u64),
    packets: Vec<(usize, u64)>,
}

impl Rec {
    /// Taxa real do dispositivo em ppm relativo ao relógio QPC.
    fn ppm(&self) -> f64 {
        let dt = (self.last.1 - self.t0) as f64 * 1e-7;
        (self.last.0 as f64 / dt / SR as f64 - 1.0) * 1e6
    }

    /// Tempo QPC (s) da amostra `i`, pelo timestamp do pacote que a contém.
    fn time_of(&self, i: usize) -> f64 {
        let k = self.packets.partition_point(|p| p.0 <= i).saturating_sub(1);
        let (start, ts) = self.packets[k];
        ts as f64 * 1e-7 + (i - start) as f64 / SR as f64
    }

    /// Instantes (s) dos transientes: |x| > 30% do pico, com 0,5 s de refratário.
    fn events(&self) -> Vec<f64> {
        let x = dc_block(&self.data);
        let peak = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let mut ev = Vec::new();
        let mut i = 0;
        while i < x.len() {
            if x[i].abs() > 0.3 * peak {
                ev.push(self.time_of(i));
                i += SR / 2;
            } else {
                i += 1;
            }
        }
        ev
    }
}

fn recorder(pattern: String, dir: Direction, loopback: bool, secs: f64) -> JoinHandle<Res<Rec>> {
    std::thread::spawn(move || {
        audio::com_init();
        let _mm = audio::mmcss_pro_audio();
        let dev = audio::find(dir, &pattern)?;
        let mut c = audio::open_capture(&dev, loopback, !loopback, 1)?;
        c.client.start_stream().map_err(|e| e.to_string())?;
        let want = (secs * SR as f64) as usize;
        let mut data = Vec::with_capacity(want + SR);
        let mut t0 = 0u64;
        let mut last = (0usize, 0u64);
        let mut packets = Vec::new();
        let mut timeouts = 0;
        while data.len() < want {
            if c.ev.wait_for_event(200).is_err() {
                timeouts += 1;
                if timeouts > 15 {
                    return Err(format!("'{}' sem dados por 3 s", c.name));
                }
                continue;
            }
            timeouts = 0;
            c.drain_ts(|s, ts| {
                if data.is_empty() {
                    t0 = ts;
                }
                last = (data.len(), ts);
                packets.push((data.len(), ts));
                data.extend_from_slice(s);
            })?;
        }
        let _ = c.client.stop_stream();
        Ok(Rec { data, t0, last, packets })
    })
}

fn player(pattern: String, sig: Arc<Vec<f32>>, looped: bool, stop: Arc<AtomicBool>) -> JoinHandle<Res<()>> {
    std::thread::spawn(move || {
        audio::com_init();
        let dev = audio::find(Direction::Render, &pattern)?;
        let mut r = audio::open_render(&dev, 2, Some(200_000))?;
        let mut buf = vec![0.0f32; r.buffer_frames as usize];
        let mut pos = 0usize;
        let mut started = false;
        while !stop.load(Relaxed) {
            let n = r.available()?;
            for v in &mut buf[..n] {
                *v = if pos < sig.len() { sig[pos] } else { 0.0 };
                pos += 1;
                if looped && pos >= sig.len() {
                    pos = 0;
                }
            }
            r.write_mono(&buf[..n])?;
            if !started {
                r.client.start_stream().map_err(|e| e.to_string())?;
                started = true;
            }
            if !looped && pos > sig.len() + SR / 2 {
                break;
            }
            let _ = r.ev.wait_for_event(500);
        }
        let _ = r.client.stop_stream();
        Ok(())
    })
}

struct App {
    child: Child,
    cfg_path: PathBuf,
    pub log: PathBuf,
}

fn start_app(name: &str, cfg: &mut Config) -> App {
    let wd = workdir();
    let cfg_path = wd.join(format!("{name}.toml"));
    let log = wd.join(format!("{name}.log"));
    let _ = std::fs::remove_file(&log);
    let _ = std::fs::remove_file(cfg_path.with_extension("stop"));
    cfg.app.log_file = log.to_string_lossy().into();
    cfg.app.stats_interval_s = 10;
    std::fs::write(&cfg_path, toml::to_string(cfg).unwrap()).unwrap();
    let exe = std::env::current_exe().unwrap().parent().unwrap().join("AMA.exe");
    let child = Command::new(exe).args(["--no-tray", "--config"]).arg(&cfg_path).spawn().expect("AMA.exe");
    App { child, cfg_path, log }
}

impl App {
    fn stop(mut self) -> String {
        std::fs::write(self.cfg_path.with_extension("stop"), "").unwrap();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if let Ok(Some(_)) = self.child.try_wait() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn cpu_s(&self) -> f64 {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::{FILETIME, HANDLE};
        let h = HANDLE(self.child.as_raw_handle() as _);
        let (mut a, mut b, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe {
            let _ = windows::Win32::System::Threading::GetProcessTimes(h, &mut a, &mut b, &mut k, &mut u);
        }
        let f = |t: FILETIME| ((t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64) as f64 * 1e-7;
        f(k) + f(u)
    }
}

/// Onde gravar a saída: CABLE Input -> captura do CABLE Output; outro -> loopback do próprio dispositivo.
fn out_recorder(out: &str, secs: f64) -> JoinHandle<Res<Rec>> {
    if out.to_lowercase().contains("cable input") {
        recorder("CABLE Output".into(), Direction::Capture, false, secs)
    } else {
        recorder(out.into(), Direction::Render, true, secs)
    }
}

fn base_cfg(out: &str) -> Config {
    let mut c = Config::default();
    c.devices.output = out.into();
    c.devices.speakers = "Speaker Out".into();
    c
}

/// Alinha duas gravações pelo timestamp QPC e devolve (a, b) começando no mesmo instante.
fn align<'a>(a: &'a Rec, b: &'a Rec) -> (&'a [f32], &'a [f32]) {
    let d = (a.t0 as i64 - b.t0 as i64) as f64 * 1e-7 * SR as f64;
    let d = d.round() as i64;
    if d >= 0 { (&a.data[..], &b.data[d as usize..]) } else { (&a.data[(-d) as usize..], &b.data[..]) }
}

/// Atraso de b em relação a a (0..max amostras) por correlação cruzada normalizada.
fn xcorr_lag(a: &[f32], b: &[f32], max: usize) -> (usize, f64) {
    let n = a.len().min(b.len()).saturating_sub(max);
    let ea: f64 = a[..n].iter().map(|&v| (v as f64).powi(2)).sum();
    let mut best = (0, 0.0);
    for l in 0..max {
        let mut s = 0.0f64;
        let mut eb = 0.0f64;
        for i in 0..n {
            s += a[i] as f64 * b[i + l] as f64;
            eb += (b[i + l] as f64).powi(2);
        }
        let c = s / (ea * eb).sqrt().max(1e-20);
        if c > best.1 {
            best = (l, c);
        }
    }
    best
}

fn cmd_list() {
    audio::com_init();
    for (dir, t) in [(Direction::Capture, "GRAVAÇÃO"), (Direction::Render, "REPRODUÇÃO")] {
        println!("{t}:");
        for d in audio::list(dir).unwrap() {
            println!("  {} {}", if d.is_default { "*" } else { " " }, d.name);
        }
    }
    let p = std::env::current_exe().unwrap().parent().unwrap().join("config.toml");
    let cfg = if p.exists() { Config::load_or_create(&p).unwrap() } else { Config::default() };
    println!("config: {}", if p.exists() { p.display().to_string() } else { "(padrão)".into() });
    let show = |dir, pat: &str, what: &str| match audio::find(dir, pat) {
        Ok(d) => println!("  {what:<22} '{pat}' -> {}", d.get_friendlyname().unwrap_or_default()),
        Err(e) => println!("  {what:<22} '{pat}' -> ERRO: {e}"),
    };
    show(Direction::Capture, &cfg.devices.mic, "mic");
    show(Direction::Render, &cfg.devices.speakers, "caixas (referência)");
    show(Direction::Render, &cfg.devices.output, "saída");
}

fn cmd_e2e(out: &str, secs: f64) {
    let far = Arc::new(testdata("far_pt.wav"));
    println!("[T9] ponta a ponta: far_pt nas caixas padrão, gravando {secs:.0} s do mic cru e da saída ('{out}', loopback)");
    let variants: [(&str, bool, bool, bool); 4] = [
        ("silencio_aec_dfn", true, true, false),
        ("sem_aec_sem_dfn", false, false, true),
        ("aec_sem_dfn", true, false, true),
        ("aec_dfn", true, true, true),
    ];
    for (name, aec, dfn, play) in variants {
        let mut cfg = base_cfg(out);
        cfg.aec.enabled = aec;
        cfg.denoise.enabled = dfn;
        let app = start_app(name, &mut cfg);
        std::thread::sleep(Duration::from_secs(3));
        let stop = Arc::new(AtomicBool::new(false));
        let pl = play.then(|| player(String::new(), far.clone(), true, stop.clone()));
        let rm = recorder(cfg.devices.mic.clone(), Direction::Capture, false, secs);
        let ro = out_recorder(out, secs);
        let (m, o) = (rm.join().unwrap(), ro.join().unwrap());
        stop.store(true, Relaxed);
        if let Some(p) = pl {
            let _ = p.join();
        }
        let log = app.stop();
        let (m, o) = match (m, o) {
            (Ok(m), Ok(o)) => (m, o),
            (a, b) => {
                println!("  {name}: erro de gravação {:?} {:?}", a.err(), b.err());
                continue;
            }
        };
        let wd = workdir();
        wav::write_mono(&wd.join(format!("{name}_mic.wav")), &m.data, SR as u32).unwrap();
        wav::write_mono(&wd.join(format!("{name}_out.wav")), &o.data, SR as u32).unwrap();
        println!("    taxa real vs QPC: mic {:+.0} ppm, saída {:+.0} ppm", m.ppm(), o.ppm());
        let skip = 5 * SR;
        let lm = rms_db(&dc_block(&m.data)[skip..]);
        let lo = rms_db(&dc_block(&o.data)[skip..]);
        let erle = log.lines().rev().find(|l| l.contains("stats:")).and_then(|l| l.split("aec atraso=").nth(1)).unwrap_or("-").to_string();
        println!(
            "  {name:<18} mic {lm:6.1} dBFS | saída {lo:6.1} dBFS | saída - mic {:+6.1} dB | AEC interno: atraso={erle}",
            lo - lm
        );
    }
}

fn cmd_latency(out: &str, mic: &str, play_dev: &str) {
    println!("[T10] latência mic -> saída ('{out}' via loopback, timestamps QPC). Sinal tocado nas caixas e captado pelo mic.");
    let mut click = vec![0.0f32; 10 * SR];
    for k in 1..9 {
        for i in 0..48 {
            click[k * SR + i] = 0.8 * (1.0 - i as f32 / 48.0) * if i % 2 == 0 { 1.0 } else { -1.0 };
        }
    }
    let near = testdata("near_en.wav");
    let speech: Vec<f32> = (0..10 * SR).map(|i| near[(i + SR) % near.len()]).collect();
    for (name, dfn, sig) in [("lat_click_sem_dfn", false, &click), ("lat_voz_com_dfn", true, &speech)] {
        let mut cfg = base_cfg(out);
        if !mic.is_empty() {
            cfg.devices.mic = mic.into();
        }
        cfg.aec.enabled = false;
        cfg.denoise.enabled = dfn;
        let app = start_app(name, &mut cfg);
        std::thread::sleep(Duration::from_secs(3));
        let stop = Arc::new(AtomicBool::new(false));
        let rm = recorder(cfg.devices.mic.clone(), Direction::Capture, false, 11.0);
        let ro = out_recorder(out, 11.0);
        std::thread::sleep(Duration::from_millis(300));
        let pl = player(play_dev.into(), Arc::new(sig.clone()), false, stop.clone());
        let (m, o) = (rm.join().unwrap(), ro.join().unwrap());
        stop.store(true, Relaxed);
        let _ = pl.join();
        let log = app.stop();
        let (Ok(m), Ok(o)) = (m, o) else {
            println!("  {name}: erro de gravação");
            continue;
        };
        let wd = workdir();
        wav::write_mono(&wd.join(format!("{name}_mic.wav")), &m.data, SR as u32).unwrap();
        wav::write_mono(&wd.join(format!("{name}_out.wav")), &o.data, SR as u32).unwrap();
        let (em, eo) = (m.events(), o.events());
        let d: Vec<f64> = em.iter().filter_map(|&t| eo.iter().find(|&&u| u > t && u < t + 0.5).map(|u| (u - t) * 1000.0)).collect();
        println!("    transientes mic={} saída={} | atrasos (ms): {}", em.len(), eo.len(), d.iter().map(|v| format!("{v:.1}")).collect::<Vec<_>>().join(" "));
        let (a, b) = align(&m, &o);
        let (a, b) = (dc_block(a), dc_block(b));
        let win = SR / 4;
        let mut lags: Vec<(usize, f64)> = (1..9)
            .map(|k| xcorr_lag(&a[k * SR..k * SR + win + SR / 5], &b[k * SR..k * SR + win + SR / 5], SR / 5))
            .collect();
        lags.sort_by(|x, y| x.0.cmp(&y.0));
        let (lag, c) = lags[lags.len() / 2];
        println!("    janelas (ms/corr): {}", lags.iter().map(|l| format!("{:.1}/{:.2}", l.0 as f64 * 1000.0 / SR as f64, l.1)).collect::<Vec<_>>().join(" "));
        println!("    taxa real vs QPC: mic {:+.0} ppm, saída {:+.0} ppm", m.ppm(), o.ppm());
        let buf = log.lines().find(|l| l.contains("latência de buffer")).unwrap_or("").to_string();
        println!(
            "  {name:<18} atraso mic -> saída = {:.1} ms (correlação {c:.2}) | mic {:.1} dBFS | {}",
            lag as f64 * 1000.0 / SR as f64,
            rms_db(&m.data),
            buf.split("] ").last().unwrap_or("")
        );
    }
}

/// Fração de quadros de 20 ms com voz no mic em que a saída caiu mais de 15 dB além da queda média.
fn voice_cut(mic: &[f32], out: &[f32]) -> (f64, f64, f64) {
    let w = SR / 50;
    let n = mic.len().min(out.len()) / w;
    let lm: Vec<f64> = (0..n).map(|k| rms_db(&mic[k * w..(k + 1) * w])).collect();
    let lo: Vec<f64> = (0..n).map(|k| rms_db(&out[k * w..(k + 1) * w])).collect();
    let mut s = lm.clone();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let floor = s[n / 10];
    let sp: Vec<usize> = (0..n).filter(|&k| lm[k] > floor + 20.0).collect();
    if sp.is_empty() {
        return (0.0, 0.0, floor);
    }
    let mut d: Vec<f64> = sp.iter().map(|&k| lm[k] - lo[k]).collect();
    d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = d[d.len() / 2];
    let cut = d.iter().filter(|&&x| x > med + 15.0).count() as f64 / d.len() as f64;
    (cut * 100.0, med, floor)
}

fn cmd_talk(out: &str) {
    let mut cfg = base_cfg(out);
    let app = start_app("talk", &mut cfg);
    std::thread::sleep(Duration::from_secs(3));
    let stop = Arc::new(AtomicBool::new(false));
    let rm = recorder(cfg.devices.mic.clone(), Direction::Capture, false, 40.0);
    let ro = out_recorder(out, 40.0);
    let say = |s: &str| println!("\n>>> {s}\n");
    say("0-10 s: FALE normalmente (só você)");
    std::thread::sleep(Duration::from_secs(10));
    say("10-20 s: 5 PALMAS perto do mic, ~1 s entre elas");
    std::thread::sleep(Duration::from_secs(10));
    say("20-40 s: CONTINUE FALANDO, vou tocar voz nas caixas junto");
    let pl = player(String::new(), Arc::new(testdata("far_pt.wav")), true, stop.clone());
    let (m, o) = (rm.join().unwrap(), ro.join().unwrap());
    stop.store(true, Relaxed);
    let _ = pl.join();
    let log = app.stop();
    say("fim, calculando");
    let (Ok(m), Ok(o)) = (m, o) else {
        println!("erro de gravação");
        return;
    };
    let wd = workdir();
    wav::write_mono(&wd.join("talk_mic.wav"), &m.data, SR as u32).unwrap();
    wav::write_mono(&wd.join("talk_out.wav"), &o.data, SR as u32).unwrap();
    let (a, b) = align(&m, &o);
    let (a, b) = (dc_block(a), dc_block(b));
    let max = SR * 3 / 10;
    let mut lags: Vec<(usize, f64)> = (1..9)
        .map(|k| {
            let s = k * SR;
            xcorr_lag(&a[s..s + SR / 4 + max], &b[s..s + SR / 4 + max], max)
        })
        .collect();
    let lag_list = lags.iter().map(|l| format!("{:.0}/{:.2}", l.0 as f64 * 1000.0 / SR as f64, l.1)).collect::<Vec<_>>().join(" ");
    lags.retain(|l| l.1 > 0.5);
    lags.sort_by(|x, y| x.0.cmp(&y.0));
    let lag = lags.get(lags.len() / 2).map(|l| l.0).unwrap_or(0);
    let em: Vec<f64> = m.events().into_iter().filter(|&t| t > m.time_of(10 * SR) && t < m.time_of(20 * SR)).collect();
    let eo = o.events();
    let claps: Vec<f64> = em.iter().filter_map(|&t| eo.iter().find(|&&u| u > t && u < t + 0.5).map(|u| (u - t) * 1000.0)).collect();
    let n = a.len().min(b.len().saturating_sub(lag));
    let bs = &b[lag..lag + n];
    let solo = voice_cut(&a[SR..10 * SR], &bs[SR..10 * SR]);
    let dt = voice_cut(&a[22 * SR..n.min(40 * SR)], &bs[22 * SR..n.min(40 * SR)]);
    let mut r = String::new();
    r += &format!("[T talk] mic USB -> micclean (AEC+DFN) -> '{out}', gravado em CABLE Output\n");
    r += &format!("  latência por correlação da voz (0-10 s): {:.1} ms | janelas ms/corr: {lag_list}\n", lag as f64 * 1000.0 / SR as f64);
    r += &format!("  latência pelas palmas: {} palmas detectadas | {}\n", claps.len(), claps.iter().map(|v| format!("{v:.0}")).collect::<Vec<_>>().join(" "));
    r += &format!("  voz sozinha: queda mediana mic->saída {:.1} dB, quadros de voz cortados {:.1}% (piso mic {:.1} dBFS)\n", solo.1, solo.0, solo.2);
    r += &format!("  voz + caixas (double talk): queda mediana {:.1} dB, quadros de voz cortados {:.1}%\n", dt.1, dt.0);
    r += &format!("  mic {:.1} dBFS | saída {:.1} dBFS\n", rms_db(&a), rms_db(&b));
    if let Some(l) = log.lines().rev().find(|l| l.contains("stats:")) {
        r += &format!("  {l}\n");
    }
    print!("{r}");
    std::fs::write(wd.join("talk_result.txt"), &r).unwrap();
}

fn cmd_longtest(out: &str, minutes: f64, play: bool) {
    println!("[T11] {minutes:.0} min rodando (saída '{out}', tocando far_pt em loop nas caixas: {play})");
    let mut cfg = base_cfg(out);
    let app = start_app("longtest", &mut cfg);
    let stop = Arc::new(AtomicBool::new(false));
    let far: Vec<f32> = testdata("far_pt.wav").iter().map(|v| v * 0.5).collect();
    let pl = play.then(|| player(String::new(), Arc::new(far), true, stop.clone()));
    let t = Instant::now();
    let c0 = app.cpu_s();
    let mut last = (Instant::now(), c0);
    let mut peak: f64 = 0.0;
    let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    while t.elapsed().as_secs_f64() < minutes * 60.0 {
        std::thread::sleep(Duration::from_secs(60));
        let c = app.cpu_s();
        let pct = (c - last.1) / last.0.elapsed().as_secs_f64() * 100.0;
        peak = peak.max(pct);
        last = (Instant::now(), c);
        println!("  {:5.1} min: CPU {pct:.1}% de 1 núcleo", t.elapsed().as_secs_f64() / 60.0);
    }
    let cpu = (app.cpu_s() - c0) / t.elapsed().as_secs_f64() * 100.0;
    stop.store(true, Relaxed);
    if let Some(p) = pl {
        let _ = p.join();
    }
    let log = app.stop();
    let stats: Vec<&str> = log.lines().filter(|l| l.contains("stats:")).collect();
    println!("  CPU média {cpu:.1}% de 1 núcleo ({:.2}% do total de {ncpu:.0} threads), pico (1 min) {peak:.1}%", cpu / ncpu);
    println!("  {} linhas de stats. Primeira e última:", stats.len());
    if let (Some(a), Some(b)) = (stats.first(), stats.last()) {
        println!("  {a}\n  {b}");
    }
    for l in log.lines().filter(|l| !l.contains("stats:")) {
        println!("  log: {l}");
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let arg = |i: usize, d: &str| a.get(i).cloned().unwrap_or_else(|| d.to_string());
    let out_default = "Steam Streaming Speakers";
    match a.get(1).map(|s| s.as_str()) {
        Some("list") => cmd_list(),
        Some("e2e") => cmd_e2e(&arg(2, out_default), arg(3, "20").parse().unwrap()),
        Some("latency") => cmd_latency(&arg(2, out_default), &arg(3, ""), &arg(4, "")),
        Some("talk") => cmd_talk(&arg(2, "CABLE Input")),
        Some("longtest") => cmd_longtest(&arg(2, out_default), arg(3, "30").parse().unwrap(), arg(4, "play") == "play"),
        Some("record") => {
            let lb = arg(4, "") == "loopback";
            let r = recorder(arg(2, ""), if lb { Direction::Render } else { Direction::Capture }, lb, arg(3, "5").parse().unwrap()).join().unwrap().unwrap();
            let p = workdir().join("record.wav");
            wav::write_mono(&p, &r.data, SR as u32).unwrap();
            println!("{} ({:.1} dBFS)", p.display(), rms_db(&r.data));
        }
        Some("play") => {
            let (sig, _) = wav::read_mono(Path::new(&arg(2, ""))).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            player(arg(3, ""), Arc::new(sig), false, stop).join().unwrap().unwrap();
        }
        _ => println!("uso: mctools list | e2e [saída] [s] | latency [saída] | longtest [saída] [min] [play|noplay] | record <disp> <s> [loopback] | play <wav> [disp]"),
    }
}
