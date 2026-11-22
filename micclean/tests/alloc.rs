mod common;
use micclean::drift::{OutPath, OutReader, RefPath};
use micclean::dsp::{Aec, Dfn, Limiter};
use micclean::{FRAME, SR};
use rtrb::RingBuffer;
use std::alloc::{GlobalAlloc, Layout, System};
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

fn count(frames: usize, mut f: impl FnMut(usize)) -> f64 {
    for i in 0..200 {
        f(i);
    }
    let a = ALLOCS.load(Relaxed);
    for i in 0..frames {
        f(200 + i);
    }
    (ALLOCS.load(Relaxed) - a) as f64 / frames as f64
}

#[test]
fn t0_alocacao_no_caminho_quente() {
    let sig = common::testdata("far_pt.wav");
    let frames = 2000;
    let mut aec = Aec::new();
    let mut x = [0.0f32; FRAME];
    let a_aec = count(frames, |i| {
        x.copy_from_slice(&sig[i * FRAME..(i + 1) * FRAME]);
        aec.process(&mut x, &sig[(i + 3) * FRAME..(i + 4) * FRAME]);
    });
    let mut lim = Limiter::new();
    let a_lim = count(frames, |i| {
        x.copy_from_slice(&sig[i * FRAME..(i + 1) * FRAME]);
        lim.process(&mut x, 4.0);
    });
    let (mut p, mut c) = RingBuffer::<f32>::new(SR / 2);
    let mut rp = RefPath::new(25.0);
    let mut rb = [0.0f32; FRAME];
    let a_ref = count(frames, |i| {
        for &s in &sig[i * FRAME..(i + 1) * FRAME] {
            let _ = p.push(s);
        }
        rp.pull(&mut c, &mut rb);
    });
    let (mut p2, mut c2) = RingBuffer::<f32>::new(SR / 2);
    let mut op = OutPath::new(20.0, SR / 2);
    let mut rd = OutReader::new(op.target);
    let mut ob = [0.0f32; FRAME];
    let a_out = count(frames, |i| {
        op.push(&sig[i * FRAME..(i + 1) * FRAME], &mut p2);
        rd.read(&mut c2, &mut ob);
    });
    let mut dfn = Dfn::new(100.0, 0.0).unwrap();
    let a_dfn = count(frames, |i| {
        x.copy_from_slice(&sig[i * FRAME..(i + 1) * FRAME]);
        dfn.process(&mut x);
    });
    println!("\n[T0] Alocações por frame de 10 ms (após aquecer 200 frames)");
    println!("  AEC (sonora): {a_aec:.2} | limiter: {a_lim:.2} | RefPath: {a_ref:.2} | OutPath+leitor: {a_out:.2} | DeepFilterNet (tract): {a_dfn:.2}");
    assert_eq!(a_aec, 0.0, "AEC aloca");
    assert_eq!(a_lim, 0.0);
    assert_eq!(a_ref, 0.0, "RefPath aloca");
    assert_eq!(a_out, 0.0, "OutPath aloca");
}
