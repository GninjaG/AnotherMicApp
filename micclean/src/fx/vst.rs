// Host VST2 baseado em EqualizerAPO helpers/VSTPluginInstance.cpp e VSTPluginLibrary.cpp (GPL-2.0).
use crate::{FRAME, SR};
use base64::Engine as _;
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCSTR, PCWSTR, w};

type Dispatcher = unsafe extern "C" fn(*mut AEffect, i32, i32, isize, *mut c_void, f32) -> isize;
type ProcessFn = unsafe extern "C" fn(*mut AEffect, *mut *mut f32, *mut *mut f32, i32);
type SetParam = unsafe extern "C" fn(*mut AEffect, i32, f32);
type GetParam = unsafe extern "C" fn(*mut AEffect, i32) -> f32;
type HostCb = unsafe extern "C" fn(*mut AEffect, i32, i32, isize, *mut c_void, f32) -> isize;
type MainFn = unsafe extern "C" fn(HostCb) -> *mut AEffect;

#[repr(C)]
pub struct AEffect {
    magic: i32,
    dispatcher: Dispatcher,
    _process: *mut c_void,
    set_parameter: SetParam,
    get_parameter: GetParam,
    num_programs: i32,
    num_params: i32,
    num_inputs: i32,
    num_outputs: i32,
    flags: i32,
    _r1: isize,
    _r2: isize,
    initial_delay: i32,
    _rq: i32,
    _oq: i32,
    _io: f32,
    _object: *mut c_void,
    _user: *mut c_void,
    unique_id: i32,
    _version: i32,
    process_replacing: ProcessFn,
    _process_double: *mut c_void,
    _future: [u8; 56],
}

const MAGIC: i32 = 0x56737450; // 'VstP'
const EFF_OPEN: i32 = 0;
const EFF_CLOSE: i32 = 1;
const EFF_GET_PARAM_NAME: i32 = 8;
const EFF_SET_SAMPLE_RATE: i32 = 10;
const EFF_SET_BLOCK_SIZE: i32 = 11;
const EFF_MAINS_CHANGED: i32 = 12;
const EFF_EDIT_GET_RECT: i32 = 13;
const EFF_EDIT_OPEN: i32 = 14;
const EFF_EDIT_CLOSE: i32 = 15;
const EFF_EDIT_IDLE: i32 = 19;
const EFF_GET_CHUNK: i32 = 23;
const EFF_SET_CHUNK: i32 = 24;
const EFF_GET_EFFECT_NAME: i32 = 45;
const EFF_GET_PRODUCT_STRING: i32 = 48;
const EFF_START_PROCESS: i32 = 71;
const EFF_STOP_PROCESS: i32 = 72;
const FLAG_HAS_EDITOR: i32 = 1;
const FLAG_CAN_REPLACING: i32 = 1 << 4;
const FLAG_PROGRAM_CHUNKS: i32 = 1 << 5;

/// Janelas de editor abertas: efeito -> HWND (só thread da interface).
fn editors() -> &'static Mutex<HashMap<usize, isize>> {
    static M: OnceLock<Mutex<HashMap<usize, isize>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

unsafe extern "C" fn host_cb(eff: *mut AEffect, op: i32, index: i32, value: isize, ptr: *mut c_void, _o: f32) -> isize {
    match op {
        0 => {
            // audioMasterAutomate
            if let Some(p) = plugins().lock().ok().and_then(|m| m.get(&(eff as usize)).cloned()) {
                p.store(true, Relaxed);
            }
            0
        }
        1 => 2400,
        2 => i32::from_be_bytes(*b"AMA ") as isize,
        4 => 0,
        7 => 0,
        15 => {
            // audioMasterSizeWindow
            if let Some(&h) = editors().lock().ok().and_then(|m| m.get(&(eff as usize)).copied()).as_ref() {
                unsafe { resize_client(HWND(h as *mut c_void), index, value as i32) };
                return 1;
            }
            0
        }
        16 => SR as isize,
        17 => FRAME as isize,
        23 => 0,
        32 | 33 => {
            if !ptr.is_null() {
                let s = b"AMA\0";
                unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), ptr as *mut u8, s.len()) };
                return 1;
            }
            0
        }
        34 => 1,
        37 => {
            if ptr.is_null() {
                return 0;
            }
            let s = unsafe { std::ffi::CStr::from_ptr(ptr as *const i8) }.to_string_lossy();
            matches!(s.as_ref(), "startStopProcess" | "sizeWindow" | "supplyIdle") as isize
        }
        38 => 1,
        41 | 42 => 0,
        _ => 0,
    }
}

/// Efeito -> flag "parâmetros mudaram" (para salvar o preset).
fn plugins() -> &'static Mutex<HashMap<usize, Arc<AtomicBool>>> {
    static M: OnceLock<Mutex<HashMap<usize, Arc<AtomicBool>>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

pub struct VstPlugin {
    eff: *mut AEffect,
    pub path: PathBuf,
    pub name: String,
    pub n_in: usize,
    pub n_out: usize,
    pub has_editor: bool,
    pub latency: usize,
    /// Plugin deu erro (saída inválida); o bloco passa o áudio direto.
    pub failed: AtomicBool,
    pub changed: Arc<AtomicBool>,
}

unsafe impl Send for VstPlugin {}
unsafe impl Sync for VstPlugin {}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl VstPlugin {
    pub fn load(path: &Path) -> Result<Arc<VstPlugin>, String> {
        if !path.exists() {
            return Err(format!("arquivo não encontrado: {}", path.display()));
        }
        unsafe {
            let wp = wide(&path.to_string_lossy());
            let lib = LoadLibraryW(PCWSTR(wp.as_ptr())).map_err(|e| format!("não carregou (precisa ser VST2 64 bits): {e}"))?;
            let f = GetProcAddress(lib, PCSTR(c"VSTPluginMain".as_ptr() as *const u8))
                .or_else(|| GetProcAddress(lib, PCSTR(c"main".as_ptr() as *const u8)))
                .ok_or("a DLL não é um plugin VST2")?;
            let main: MainFn = std::mem::transmute(f);
            let eff = main(host_cb);
            if eff.is_null() || (*eff).magic != MAGIC {
                return Err("o plugin não inicializou".into());
            }
            let e = &*eff;
            if e.flags & FLAG_CAN_REPLACING == 0 {
                return Err("plugin sem processReplacing (não suportado)".into());
            }
            let changed = Arc::new(AtomicBool::new(false));
            plugins().lock().unwrap().insert(eff as usize, changed.clone());
            (e.dispatcher)(eff, EFF_OPEN, 0, 0, std::ptr::null_mut(), 0.0);
            (e.dispatcher)(eff, EFF_SET_SAMPLE_RATE, 0, 0, std::ptr::null_mut(), SR as f32);
            (e.dispatcher)(eff, EFF_SET_BLOCK_SIZE, 0, FRAME as isize, std::ptr::null_mut(), 0.0);
            let mut name = Self::string(eff, EFF_GET_EFFECT_NAME);
            if name.is_empty() {
                name = Self::string(eff, EFF_GET_PRODUCT_STRING);
            }
            if name.is_empty() {
                name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            }
            (e.dispatcher)(eff, EFF_MAINS_CHANGED, 0, 1, std::ptr::null_mut(), 0.0);
            (e.dispatcher)(eff, EFF_START_PROCESS, 0, 0, std::ptr::null_mut(), 0.0);
            Ok(Arc::new(VstPlugin {
                eff,
                path: path.to_path_buf(),
                name,
                n_in: e.num_inputs.max(0) as usize,
                n_out: e.num_outputs.max(0) as usize,
                has_editor: e.flags & FLAG_HAS_EDITOR != 0,
                latency: e.initial_delay.max(0) as usize,
                failed: AtomicBool::new(false),
                changed,
            }))
        }
    }

    unsafe fn string(eff: *mut AEffect, op: i32) -> String {
        let mut b = [0u8; 256];
        unsafe { ((*eff).dispatcher)(eff, op, 0, 0, b.as_mut_ptr() as *mut c_void, 0.0) };
        b[255] = 0;
        let n = b.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf8_lossy(&b[..n]).trim().to_string()
    }

    fn param_name(&self, i: i32) -> String {
        let mut b = [0u8; 256];
        unsafe { ((*self.eff).dispatcher)(self.eff, EFF_GET_PARAM_NAME, i, 0, b.as_mut_ptr() as *mut c_void, 0.0) };
        b[255] = 0;
        let n = b.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf8_lossy(&b[..n]).into_owned()
    }

    fn uses_chunks(&self) -> bool {
        unsafe { (*self.eff).flags & FLAG_PROGRAM_CHUNKS != 0 }
    }

    /// Igual ao writeToEffect do EAPO: chunk base64 se o plugin usa chunks, senão parâmetros por nome.
    pub fn set_state(&self, chunk: Option<&str>, params: &BTreeMap<String, f32>) {
        unsafe {
            let e = &*self.eff;
            if self.uses_chunks() {
                if let Some(c) = chunk.filter(|c| !c.is_empty()) {
                    if let Ok(mut buf) = base64::engine::general_purpose::STANDARD.decode(c.trim()) {
                        (e.dispatcher)(self.eff, EFF_SET_CHUNK, 1, buf.len() as isize, buf.as_mut_ptr() as *mut c_void, 0.0);
                    }
                }
            } else {
                for i in 0..e.num_params {
                    if let Some(v) = params.get(&self.param_name(i)) {
                        (e.set_parameter)(self.eff, i, *v);
                    }
                }
            }
        }
    }

    pub fn get_state(&self) -> (Option<String>, BTreeMap<String, f32>) {
        let mut params = BTreeMap::new();
        unsafe {
            let e = &*self.eff;
            if self.uses_chunks() {
                let mut p: *mut u8 = std::ptr::null_mut();
                let n = (e.dispatcher)(self.eff, EFF_GET_CHUNK, 1, 0, &mut p as *mut *mut u8 as *mut c_void, 0.0);
                if n > 0 && !p.is_null() {
                    let s = std::slice::from_raw_parts(p, n as usize);
                    return (Some(base64::engine::general_purpose::STANDARD.encode(s)), params);
                }
                return (None, params);
            }
            for i in 0..e.num_params {
                params.insert(self.param_name(i), (e.get_parameter)(self.eff, i));
            }
        }
        (None, params)
    }

    pub fn editor_open(&self) -> bool {
        editors().lock().map(|m| m.contains_key(&(self.eff as usize))).unwrap_or(false)
    }

    /// Abre a janela original do plugin (thread da interface).
    pub fn open_editor(&self) -> Result<(), String> {
        if !self.has_editor {
            return Err("este plugin não tem interface própria".into());
        }
        if let Some(&h) = editors().lock().unwrap().get(&(self.eff as usize)) {
            unsafe {
                let _ = ShowWindow(HWND(h as *mut c_void), SW_RESTORE);
                let _ = SetForegroundWindow(HWND(h as *mut c_void));
            }
            return Ok(());
        }
        unsafe {
            register_class();
            let title = wide(&self.name);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("AMA_VST"),
                PCWSTR(title.as_ptr()),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                300,
                None,
                None,
                None,
                None,
            )
            .map_err(|e| e.to_string())?;
            dark_title(hwnd);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, self.eff as isize);
            editors().lock().unwrap().insert(self.eff as usize, hwnd.0 as isize);
            let d = (*self.eff).dispatcher;
            d(self.eff, EFF_EDIT_OPEN, 0, 0, hwnd.0, 0.0);
            let mut r: *mut [i16; 4] = std::ptr::null_mut();
            d(self.eff, EFF_EDIT_GET_RECT, 0, 0, &mut r as *mut *mut [i16; 4] as *mut c_void, 0.0);
            if !r.is_null() {
                let [top, left, bottom, right] = *r;
                resize_client(hwnd, (right - left) as i32, (bottom - top) as i32);
            }
            SetTimer(Some(hwnd), 1, 30, None);
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        Ok(())
    }

    pub fn close_editor(&self) {
        if let Some(h) = editors().lock().unwrap().get(&(self.eff as usize)).copied() {
            unsafe {
                let _ = SendMessageW(HWND(h as *mut c_void), WM_CLOSE, None, None);
            }
        }
    }

    /// Processa um quadro mono (thread de áudio). Entradas estéreo recebem o mesmo sinal.
    pub fn process(&self, io: &mut VstIo, x: &mut [f32]) {
        if self.failed.load(Relaxed) || self.n_out == 0 {
            return;
        }
        for (i, b) in io.ins.iter_mut().enumerate() {
            if i < 2 {
                b.copy_from_slice(x);
            } else {
                b.fill(0.0);
            }
        }
        unsafe { ((*self.eff).process_replacing)(self.eff, io.inp.as_mut_ptr(), io.outp.as_mut_ptr(), FRAME as i32) };
        let k = self.n_out.min(2);
        let mut ok = true;
        for (j, v) in x.iter_mut().enumerate() {
            let mut s = io.outs[0][j];
            if k == 2 {
                s = 0.5 * (s + io.outs[1][j]);
            }
            if !s.is_finite() || s.abs() > 1e4 {
                ok = false;
                break;
            }
            *v = s;
        }
        if !ok {
            self.failed.store(true, Relaxed);
        }
    }
}

impl Drop for VstPlugin {
    fn drop(&mut self) {
        self.close_editor();
        unsafe {
            let d = (*self.eff).dispatcher;
            d(self.eff, EFF_STOP_PROCESS, 0, 0, std::ptr::null_mut(), 0.0);
            d(self.eff, EFF_MAINS_CHANGED, 0, 0, std::ptr::null_mut(), 0.0);
            d(self.eff, EFF_CLOSE, 0, 0, std::ptr::null_mut(), 0.0);
        }
        plugins().lock().unwrap().remove(&(self.eff as usize));
    }
}

/// Buffers por instância de bloco (alocados fora da thread de áudio).
pub struct VstIo {
    ins: Vec<Vec<f32>>,
    outs: Vec<Vec<f32>>,
    inp: Vec<*mut f32>,
    outp: Vec<*mut f32>,
}
unsafe impl Send for VstIo {}

impl VstIo {
    pub fn new(p: &VstPlugin) -> VstIo {
        let mut ins: Vec<Vec<f32>> = (0..p.n_in.max(1)).map(|_| vec![0.0; FRAME]).collect();
        let mut outs: Vec<Vec<f32>> = (0..p.n_out.max(1)).map(|_| vec![0.0; FRAME]).collect();
        let inp = ins.iter_mut().map(|b| b.as_mut_ptr()).collect();
        let outp = outs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        VstIo { ins, outs, inp, outp }
    }
}

unsafe fn resize_client(h: HWND, w: i32, ht: i32) {
    unsafe {
        let mut r = RECT { left: 0, top: 0, right: w, bottom: ht };
        let st = WINDOW_STYLE(GetWindowLongW(h, GWL_STYLE) as u32);
        let _ = AdjustWindowRectEx(&mut r, st, false, WINDOW_EX_STYLE(0));
        let _ = SetWindowPos(h, None, 0, 0, r.right - r.left, r.bottom - r.top, SWP_NOMOVE | SWP_NOZORDER);
    }
}

pub fn dark_title(h: HWND) {
    use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    let on: i32 = 1;
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &on as *const i32 as *const c_void, 4);
    }
}

unsafe fn register_class() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: w!("AMA_VST"),
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(
                windows::Win32::Graphics::Gdi::GetStockObject(windows::Win32::Graphics::Gdi::BLACK_BRUSH).0,
            ),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
    });
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let eff = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut AEffect;
        match msg {
            WM_TIMER if !eff.is_null() => {
                ((*eff).dispatcher)(eff, EFF_EDIT_IDLE, 0, 0, std::ptr::null_mut(), 0.0);
                LRESULT(0)
            }
            WM_CLOSE => {
                if !eff.is_null() {
                    let _ = KillTimer(Some(h), 1);
                    ((*eff).dispatcher)(eff, EFF_EDIT_CLOSE, 0, 0, std::ptr::null_mut(), 0.0);
                    editors().lock().unwrap().remove(&(eff as usize));
                    SetWindowLongPtrW(h, GWLP_USERDATA, 0);
                }
                let _ = DestroyWindow(h);
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

/// Lista DLLs nas pastas (sem carregar).
pub fn scan(dirs: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(d: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && depth < 3 {
                walk(&p, depth + 1, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")) {
                out.push(p);
            }
        }
    }
    for d in dirs {
        walk(Path::new(d), 0, &mut out);
    }
    out.sort();
    out.dedup();
    out
}
