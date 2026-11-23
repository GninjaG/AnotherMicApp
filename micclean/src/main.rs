#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

use app::presets::Store;
use app::{AmaApp, Boot, Shared};
use micclean::config::exe_dir;
use micclean::dsp::build_chain;
use micclean::engine::Engine;
use micclean::fx::chain::{Builder, Preset};
use micclean::log;
use std::path::PathBuf;
use std::sync::Arc;

fn message(text: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    use windows::core::HSTRING;
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), &HSTRING::from("AMA"), MB_OK | MB_ICONERROR);
    }
}

/// Só uma instância: a segunda apenas traz a janela da primeira para a frente.
fn already_running(after_update: bool) -> bool {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    use windows::core::w;
    unsafe {
        let mut tries = if after_update { 150 } else { 0 };
        loop {
            let h = CreateMutexW(None, true, w!("Local\\AMA_AnotherMicApp"));
            if GetLastError() != ERROR_ALREADY_EXISTS || tries == 0 {
                break;
            }
            // a versão anterior ainda está fechando
            if let Ok(h) = h {
                let _ = windows::Win32::Foundation::CloseHandle(h);
            }
            tries -= 1;
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(h) = FindWindowW(None, w!("AMA")) {
                app::sys::raise(h.0 as isize);
            }
            return true;
        }
    }
    false
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |a: &str| args.iter().any(|x| x == a);
    let base = exe_dir();

    // modo sem janela (usado pelos testes de ponta a ponta): roda até <config>.stop existir
    if has("--no-tray") {
        let cfg_path = args.iter().position(|a| a == "--config").and_then(|i| args.get(i + 1)).map(PathBuf::from).unwrap_or_else(|| base.join("ama.toml"));
        let cfg = match micclean::config::Config::load_or_create(&cfg_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("config: {e}");
                return;
            }
        };
        let b = cfg_path.parent().map(|p| p.to_path_buf()).unwrap_or_else(exe_dir);
        micclean::logger::init(&cfg.resolve(&b, &cfg.app.log_file), cfg!(debug_assertions));
        let preset = micclean::dsp::legacy_preset(&cfg);
        let mut vst = Default::default();
        let mut bld = Builder { vst: &mut vst, vol_db: 0.0, base: &b };
        let (chain, _) = build_chain(&preset, &mut bld);
        let engine = match Engine::start(&cfg, &preset, chain) {
            Ok(e) => e,
            Err(e) => {
                log!("falha ao iniciar: {e}");
                return;
            }
        };
        let stopf = cfg_path.with_extension("stop");
        while !stopf.exists() {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let _ = std::fs::remove_file(&stopf);
        engine.stop();
        return;
    }

    if let Some(i) = args.iter().position(|a| a == app::cable::ARG) {
        std::process::exit(app::cable::elevated_main(args.get(i + 1).map(|s| s.as_str()).unwrap_or("")));
    }
    if already_running(has("--after-update")) {
        return;
    }
    let start_hidden = has("--tray");
    let base = micclean::config::data_dir();
    let moved = app::move_old_data(&exe_dir(), &base);
    let (mut cfg, cfg_path) = app::load_config(&base);
    app::i18n::set(&cfg.app.language);
    micclean::logger::init(&cfg.resolve(&base, &cfg.app.log_file), cfg!(debug_assertions));
    log!("AMA {} iniciando{}", env!("CARGO_PKG_VERSION"), if start_hidden { " na bandeja" } else { "" });
    if !moved.is_empty() {
        log!("movido de {} para {}: {}", exe_dir().display(), base.display(), moved.join(", "));
    }
    let store = Store::new(&base);
    let mut preset = None;
    if !cfg.app.preset.is_empty() {
        match store.load(&cfg.app.preset) {
            Ok(p) => preset = Some(p),
            Err(e) => log!("preset '{}' não abriu: {e}", cfg.app.preset),
        }
    }
    let preset = preset.or_else(|| store.list().first().and_then(|n| store.load(n).ok())).unwrap_or_else(|| {
        let mut p = Preset::default_chain("Padrão");
        for b in &mut p.blocks {
            if let micclean::fx::chain::Kind::Denoise { atten_db, .. } = &mut b.kind {
                *atten_db = cfg.denoise.atten_limit_db;
            }
        }
        let _ = store.save(&p);
        p
    });
    cfg.app.preset = preset.name.clone();
    let _ = cfg.save(&cfg_path);
    if cfg.app.autostart {
        app::sys::set_autostart(true);
    }

    let shared = Arc::new(Shared::default());
    // COM em MTA fica fora da thread da janela (o winit precisa de STA)
    let vol = std::thread::spawn(app::sys::master_volume_db).join().ok().flatten().unwrap_or(0.0);
    shared.vol_db.set(vol);
    let mut vst = Default::default();
    let mut bld = Builder { vst: &mut vst, vol_db: vol, base: &base };
    let (chain, errs) = build_chain(&preset, &mut bld);
    for (id, e) in &errs {
        log!("bloco {id}: {e}");
    }
    let engine = match Engine::start(&cfg, &preset, chain) {
        Ok(e) => Arc::new(e),
        Err(e) => {
            log!("falha ao iniciar: {e}");
            message(&format!("O motor de áudio não iniciou:\n{e}"));
            return;
        }
    };
    {
        let (e, s) = (engine.clone(), shared.clone());
        std::thread::Builder::new().name("ctl".into()).spawn(move || app::ctl_thread(e, s)).ok();
    }

    let size = cfg.app.window;
    let icon = egui::IconData { rgba: app::sys::icon_rgba(64), width: 64, height: 64 };
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AMA")
            .with_inner_size([size[0].max(960.0), size[1].max(600.0)])
            .with_min_inner_size([960.0, 600.0])
            .with_icon(icon)
            .with_visible(!start_hidden),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let boot = Boot { cfg, cfg_path, base, engine: engine.clone(), shared: shared.clone(), store, preset, vst, errs };
    let r = eframe::run_native("AMA", opts, Box::new(move |cc| Ok(Box::new(AmaApp::new(cc, boot)))));
    if let Err(e) = r {
        log!("janela: {e}");
        message(&format!("A janela não abriu:\n{e}"));
    }
    shared.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    engine.stop();
}
