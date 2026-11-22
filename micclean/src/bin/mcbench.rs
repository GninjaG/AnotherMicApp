use micclean::config::Config;
use micclean::drift::{OutPath, OutReader, RefPath};
use micclean::dsp::{Aec, Dfn, Limiter, Pipeline};
use micclean::{FRAME, SR};
use rtrb::RingBuffer;
use std::path::PathBuf;
use std::time::Instant;

fn report(name: &str, mut v: Vec<f64>) -> (f64, f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let p99 = v[(v.len() as f64 * 0.99) as usize];
    let max = *v.last().unwrap();
    println!("  {name:<28} média {mean:7.3} ms | p99 {p99:7.3} ms | máx {max:7.3} ms | RTF {:.3}", mean / 10.0);
    (mean, p99, max)
}

fn main() {
    unsafe {
        use windows::Win32::System::Threading::*;
        let _ = SetThreadAffinityMask(GetCurrentThread(), 1 << 2);
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    }
    let td = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let (far, _) = micclean::wav::read_mono(&td.join("far_pt.wav")).expect("testdata");
    let (near, _) = micclean::wav::read_mono(&td.join("near_en.wav")).expect("testdata");
    let n = far.len() / FRAME * FRAME;
    let mic: Vec<f32> = (0..n)
        .map(|i| {
            let e = if i >= 2400 { 0.3 * far[i - 2400] } else { 0.0 };
            e + near[i % near.len()] + 0.01 * ((i * 7919 % 1000) as f32 / 500.0 - 1.0)
        })
        .collect();
    let frames = n / FRAME;
    println!("[T7] Benchmark: {frames} frames de 10 ms ({:.0} s), 1 núcleo fixo (CPU 2), prioridade crítica", n as f64 / SR as f64);
    let warm = 100;

    let mut aec = Aec::new();
    let mut t = Vec::new();
    let mut x = [0.0f32; FRAME];
    for k in 0..frames {
        x.copy_from_slice(&mic[k * FRAME..(k + 1) * FRAME]);
        let s = Instant::now();
        aec.process(&mut x, &far[k * FRAME..(k + 1) * FRAME]);
        if k >= warm {
            t.push(s.elapsed().as_secs_f64() * 1e3);
        }
    }
    report("AEC3 (sonora)", t);

    let mut dfn = Dfn::new(100.0, 0.0).unwrap();
    let mut t = Vec::new();
    for k in 0..frames {
        x.copy_from_slice(&mic[k * FRAME..(k + 1) * FRAME]);
        let s = Instant::now();
        dfn.process(&mut x);
        if k >= warm {
            t.push(s.elapsed().as_secs_f64() * 1e3);
        }
    }
    report("DeepFilterNet3 (tract)", t);

    let mut lim = Limiter::new();
    let mut t = Vec::new();
    for k in 0..frames {
        x.copy_from_slice(&mic[k * FRAME..(k + 1) * FRAME]);
        let s = Instant::now();
        lim.process(&mut x, 2.0);
        if k >= warm {
            t.push(s.elapsed().as_secs_f64() * 1e3);
        }
    }
    report("ganho + limiter", t);

    let (mut rp, mut rc) = RingBuffer::<f32>::new(SR / 2);
    let mut refp = RefPath::new(25.0);
    let mut rb = [0.0f32; FRAME];
    let mut t = Vec::new();
    for k in 0..frames {
        for &v in &far[k * FRAME..(k + 1) * FRAME] {
            let _ = rp.push(v);
        }
        let s = Instant::now();
        refp.pull(&mut rc, &mut rb);
        if k >= warm {
            t.push(s.elapsed().as_secs_f64() * 1e3);
        }
    }
    report("resampler referência", t);

    let (mut op, mut oc) = RingBuffer::<f32>::new(SR / 2);
    let mut outp = OutPath::new(20.0, SR / 2);
    let mut rd = OutReader::new(outp.target);
    let mut ob = [0.0f32; FRAME];
    let mut t = Vec::new();
    for k in 0..frames {
        let s = Instant::now();
        outp.push(&mic[k * FRAME..(k + 1) * FRAME], &mut op);
        if k >= warm {
            t.push(s.elapsed().as_secs_f64() * 1e3);
        }
        rd.read(&mut oc, &mut ob);
    }
    report("resampler saída", t);

    // Frame completo, como na thread do mic
    let cfg = Config::default();
    let mut pipe = Pipeline::from_config(&cfg).unwrap();
    let (mut rp, mut rc) = RingBuffer::<f32>::new(SR / 2);
    let (mut op, mut oc) = RingBuffer::<f32>::new(SR / 2);
    let mut refp = RefPath::new(cfg.buffers.ref_ms);
    let mut outp = OutPath::new(cfg.buffers.out_ms, SR / 2);
    let mut rd = OutReader::new(outp.target);
    let mut t = Vec::new();
    for k in 0..frames {
        for &v in &far[k * FRAME..(k + 1) * FRAME] {
            let _ = rp.push(v);
        }
        x.copy_from_slice(&mic[k * FRAME..(k + 1) * FRAME]);
        let s = Instant::now();
        refp.pull(&mut rc, &mut rb);
        pipe.process(&mut x, &rb);
        outp.push(&x, &mut op);
        if k >= warm {
            t.push(s.elapsed().as_secs_f64() * 1e3);
        }
        rd.read(&mut oc, &mut ob);
    }
    let (mean, p99, max) = report("TOTAL (frame completo)", t);
    let ok = mean < 3.0;
    println!(
        "  meta total < 3 ms (RTF < 0,3): {} | folga no pior caso: {:.1} ms de 10 ms | p99 {:.2} ms",
        if ok { "PASSOU" } else { "FALHOU" },
        10.0 - max,
        p99
    );
    std::process::exit(if ok { 0 } else { 1 });
}
