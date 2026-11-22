//! Bandeja, atalhos globais, autostart, volume do Windows e janela.
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use micclean::log;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Receiver, Sender, channel};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum SysEv {
    Show,
    Quit,
    Mute,
    Bypass,
    Preset(String),
}

pub fn icon_rgba(n: u32) -> Vec<u8> {
    // quadrado arredondado escuro com três barras de nível em rosa
    let mut px = Vec::with_capacity((n * n * 4) as usize);
    let s = n as f32 / 32.0;
    let bars = [(9.0, 12.0, 20.0), (14.5, 7.0, 25.0), (20.0, 10.0, 22.0)];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 / s + 0.5 / s, y as f32 / s + 0.5 / s);
            let r = 7.0;
            let dx = (fx - 16.0).abs() - (16.0 - r);
            let dy = (fy - 16.0).abs() - (16.0 - r);
            let d = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - r;
            if d > 0.5 {
                px.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let a = ((0.5 - d).clamp(0.0, 1.0) * 255.0) as u8;
            let on = bars.iter().any(|&(bx, top, bot)| (fx - bx - 1.5).abs() < 1.6 && fy > top && fy < bot);
            if on {
                px.extend_from_slice(&[0xe2, 0x4a, 0x76, a]);
            } else {
                px.extend_from_slice(&[0x1f, 0x19, 0x1f, a]);
            }
        }
    }
    px
}

pub struct Tray {
    _icon: TrayIcon,
    pub mute: CheckMenuItem,
    pub bypass: CheckMenuItem,
    presets: Submenu,
    open: MenuItem,
    quit: MenuItem,
    preset_items: Vec<(CheckMenuItem, String)>,
}

pub struct Sys {
    pub rx: Receiver<SysEv>,
    tx: Sender<SysEv>,
    pub tray: Option<Tray>,
    hk: Option<GlobalHotKeyManager>,
    hk_ids: Arc<Mutex<(u32, u32)>>,
    hk_keys: Vec<HotKey>,
    preset_ids: Option<Arc<Mutex<Vec<(MenuId, String)>>>>,
    pub hwnd: Arc<AtomicIsize>,
}

impl Sys {
    pub fn new(ctx: &egui::Context) -> Sys {
        let (tx, rx) = channel();
        let c = ctx.clone();
        let t = tx.clone();
        let hk_ids: Arc<Mutex<(u32, u32)>> = Default::default();
        let ids = hk_ids.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state != HotKeyState::Pressed {
                return;
            }
            let (m, b) = *ids.lock().unwrap();
            let ev = if e.id == m && m != 0 {
                SysEv::Mute
            } else if e.id == b && b != 0 {
                SysEv::Bypass
            } else {
                return;
            };
            let _ = t.send(ev);
            c.request_repaint();
        }));
        Sys { rx, tx, tray: None, hk: GlobalHotKeyManager::new().ok(), hk_ids, hk_keys: vec![], preset_ids: None, hwnd: Default::default() }
    }

    pub fn build_tray(&mut self, ctx: &egui::Context, presets: &[String], current: &str) {
        let menu = Menu::new();
        let open = MenuItem::new("", true, None);
        let mute = CheckMenuItem::new("", true, false, None);
        let bypass = CheckMenuItem::new("", true, false, None);
        let presets_m = Submenu::new("Preset", true);
        let quit = MenuItem::new("", true, None);
        let _ = menu.append(&open);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&mute);
        let _ = menu.append(&bypass);
        let _ = menu.append(&presets_m);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&quit);
        let icon = Icon::from_rgba(icon_rgba(32), 32, 32).expect("icon");
        let Ok(ti) = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("AMA").with_icon(icon).build() else {
            log!("bandeja indisponível");
            return;
        };
        let (oid, mid, bid, qid) = (open.id().clone(), mute.id().clone(), bypass.id().clone(), quit.id().clone());
        let tx = self.tx.clone();
        let c = ctx.clone();
        let preset_ids: Arc<Mutex<Vec<(MenuId, String)>>> = Default::default();
        let hwnd = self.hwnd.clone();
        let pids = preset_ids.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let ev = if e.id == oid {
                raise(hwnd.load(Relaxed));
                Some(SysEv::Show)
            } else if e.id == mid {
                Some(SysEv::Mute)
            } else if e.id == bid {
                Some(SysEv::Bypass)
            } else if e.id == qid {
                Some(SysEv::Quit)
            } else {
                pids.lock().unwrap().iter().find(|(i, _)| *i == e.id).map(|(_, n)| SysEv::Preset(n.clone()))
            };
            if let Some(ev) = ev {
                let _ = tx.send(ev);
                c.request_repaint();
            }
        }));
        let tx = self.tx.clone();
        let c = ctx.clone();
        let hwnd = self.hwnd.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                raise(hwnd.load(Relaxed));
                let _ = tx.send(SysEv::Show);
                c.request_repaint();
            }
        }));
        let mut t = Tray { _icon: ti, mute, bypass, presets: presets_m, open, quit, preset_items: vec![] };
        t.set_texts();
        t.set_presets(presets, current, &preset_ids);
        self.preset_ids = Some(preset_ids);
        self.tray = Some(t);
    }

    pub fn set_language(&mut self) {
        if let Some(t) = self.tray.as_mut() {
            t.set_texts();
        }
    }

    pub fn refresh_presets(&mut self, presets: &[String], current: &str) {
        if let (Some(t), Some(ids)) = (self.tray.as_mut(), self.preset_ids.as_ref()) {
            t.set_presets(presets, current, ids);
        }
    }

    /// (mutar, bypass). Retorna erro legível se algum atalho não registrou.
    pub fn set_hotkeys(&mut self, mute: &str, bypass: &str) -> Result<(), String> {
        let Some(m) = self.hk.as_ref() else { return Err(t!("atalhos globais indisponíveis", "global hotkeys unavailable").into()) };
        let _ = m.unregister_all(&self.hk_keys);
        self.hk_keys.clear();
        let mut ids = (0, 0);
        let mut err = vec![];
        for (i, s) in [mute, bypass].iter().enumerate() {
            if s.trim().is_empty() {
                continue;
            }
            match parse_hotkey(s) {
                Some(hk) => match m.register(hk) {
                    Ok(()) => {
                        if i == 0 { ids.0 = hk.id() } else { ids.1 = hk.id() }
                        self.hk_keys.push(hk);
                    }
                    Err(e) => {
                        let e = e.to_string();
                        if e.contains("already registered") {
                            err.push(tr!("{s} já está em uso por outro programa. Escolha outro atalho.", "{s} is already used by another program. Pick another hotkey."));
                        } else {
                            err.push(format!("{s}: {e}"));
                        }
                    }
                },
                None => err.push(tr!("{s}: atalho inválido", "{s}: invalid hotkey")),
            }
        }
        *self.hk_ids.lock().unwrap() = ids;
        if err.is_empty() { Ok(()) } else { Err(err.join("; ")) }
    }
}

impl Tray {
    fn set_texts(&mut self) {
        self.open.set_text(t!("Abrir AMA", "Open AMA"));
        self.mute.set_text(t!("Mutar mic", "Mute mic"));
        self.bypass.set_text(t!("Bypass (mic cru)", "Bypass (raw mic)"));
        self.quit.set_text(t!("Sair", "Quit"));
    }

    fn set_presets(&mut self, presets: &[String], current: &str, ids: &Mutex<Vec<(MenuId, String)>>) {
        for (it, _) in self.preset_items.drain(..) {
            let _ = self.presets.remove(&it);
        }
        let mut v = ids.lock().unwrap();
        v.clear();
        for p in presets {
            let it = CheckMenuItem::new(p, true, p == current, None);
            let _ = self.presets.append(&it);
            v.push((it.id().clone(), p.clone()));
            self.preset_items.push((it, p.clone()));
        }
    }
}

pub fn parse_hotkey(s: &str) -> Option<HotKey> {
    let mut mods = Modifiers::empty();
    let mut code = None;
    for part in s.split('+').map(|p| p.trim()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "alt" => mods |= Modifiers::ALT,
            "shift" => mods |= Modifiers::SHIFT,
            "win" | "super" => mods |= Modifiers::META,
            k => code = key_code(k),
        }
    }
    Some(HotKey::new(Some(mods).filter(|m| !m.is_empty()), code?))
}

fn key_code(k: &str) -> Option<Code> {
    use Code::*;
    let letters = [KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ];
    let digits = [Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9];
    let f = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    let b = k.as_bytes();
    if b.len() == 1 && b[0].is_ascii_lowercase() {
        return Some(letters[(b[0] - b'a') as usize]);
    }
    if b.len() == 1 && b[0].is_ascii_digit() {
        return Some(digits[(b[0] - b'0') as usize]);
    }
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
        return f.get(n.wrapping_sub(1)).copied();
    }
    match k {
        "pause" => Some(Pause),
        "insert" => Some(Insert),
        "home" => Some(Home),
        "end" => Some(End),
        "pageup" => Some(PageUp),
        "pagedown" => Some(PageDown),
        _ => None,
    }
}

/// Texto do atalho a partir de uma tecla do egui (para capturar na tela de ajustes).
pub fn format_hotkey(m: egui::Modifiers, key: egui::Key) -> Option<String> {
    let name = key.name();
    let ok = name.len() == 1 || (name.starts_with('F') && name.len() <= 3) || matches!(name, "Pause" | "Insert" | "Home" | "End" | "PageUp" | "PageDown");
    if !ok || !(m.ctrl || m.alt || m.shift || name.starts_with('F')) {
        return None;
    }
    let mut s = String::new();
    if m.ctrl {
        s.push_str("Ctrl+");
    }
    if m.alt {
        s.push_str("Alt+");
    }
    if m.shift {
        s.push_str("Shift+");
    }
    s.push_str(name);
    Some(s)
}

fn startup_lnk() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Start Menu\Programs\Startup\AMA.lnk"))
}

pub fn set_autostart(on: bool) {
    let Some(lnk) = startup_lnk() else { return };
    if on {
        let exe = std::env::current_exe().unwrap_or_default();
        let ps = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Arguments='--tray';$s.WorkingDirectory='{}';$s.Save()",
            lnk.display(),
            exe.display(),
            micclean::config::exe_dir().display()
        );
        use std::os::windows::process::CommandExt;
        let ok = std::process::Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps])
            .creation_flags(0x0800_0000)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        log!("autostart: atalho em {} ({})", lnk.display(), if ok { "ok" } else { "falhou" });
    } else if lnk.exists() {
        let _ = std::fs::remove_file(&lnk);
        log!("autostart: atalho removido");
    }
}

/// Volume mestre da reprodução padrão em dB (usado pelo Loudness correction, como no EAPO).
pub fn master_volume_db() -> Option<f64> {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender};
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        let vol: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None).ok()?;
        vol.GetMasterVolumeLevel().ok().map(|v| v as f64)
    }
}

pub fn hwnd_of(cc: &eframe::CreationContext) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// Mostra a janela direto pelo Win32 (funciona mesmo com o egui parado enquanto oculto).
pub fn raise(h: isize) {
    if h == 0 {
        return;
    }
    use windows::Win32::UI::WindowsAndMessaging::*;
    let h = windows::Win32::Foundation::HWND(h as *mut std::ffi::c_void);
    unsafe {
        let _ = ShowWindow(h, SW_SHOW);
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let _ = SetForegroundWindow(h);
    }
}

pub fn show_window(hwnd: isize, ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    raise(hwnd);
    ctx.request_repaint();
}

pub fn dark_title(hwnd: isize) {
    if hwnd != 0 {
        micclean::fx::vst::dark_title(windows::Win32::Foundation::HWND(hwnd as *mut std::ffi::c_void));
    }
}

pub fn hide_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
}
