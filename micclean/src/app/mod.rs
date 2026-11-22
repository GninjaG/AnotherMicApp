#[macro_use]
pub mod i18n;
pub mod cable;
pub mod cards;
pub mod graph;
pub mod presets;
pub mod settings;
pub mod sys;
pub mod theme;
pub mod update;
pub mod wizard;

use cards::{Cards, Edit};
use egui::{Color32, RichText, Stroke, vec2};
use graph::{Graph, GraphEdit, Meter, Spectrum};
use micclean::config::Config;
use micclean::dsp::build_chain;
use micclean::engine::Engine;
use micclean::fx::biquad::Coefs;
use micclean::fx::chain::{BlockCfg, Builder, Cmd, Kind, Preset, db};
use micclean::fx::vst::{self, VstPlugin};
use micclean::fx::{eapo, loudness};
use micclean::stats::{AtomicF64, get, take_peak};
use micclean::{SR, log};
use presets::Store;
use settings::{SetEdit, Settings};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use sys::{Sys, SysEv};
use theme::*;

/// Estado lido pela thread de controle (volume do Windows para o Loudness correction).
#[derive(Default)]
pub struct Shared {
    pub loud: Mutex<Vec<(u32, f64, f64, f64)>>,
    pub vol_db: AtomicF64,
    pub stop: AtomicBool,
}

pub fn ctl_thread(engine: Arc<Engine>, sh: Arc<Shared>) {
    micclean::audio::com_init();
    let mut last = f64::NAN;
    while !sh.stop.load(Relaxed) {
        engine.collect();
        if let Some(v) = sys::master_volume_db() {
            if !(v - last).abs().le(&0.05) {
                last = v;
                sh.vol_db.set(v);
                for &(id, a, b, c) in sh.loud.lock().unwrap().iter() {
                    engine.send(Cmd::Loud(id, loudness::coefs(a, b, c, v, SR as f64)));
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

enum Upd {
    None,
    Available(update::Release),
    Installing(String),
}

enum UpdMsg {
    Found(update::Release),
    Installed(std::path::PathBuf),
    Failed(String),
}

enum NameKind {
    New,
    Duplicate,
    Rename,
}

enum Dialog {
    Name(NameKind, String),
    Info(String, Vec<String>),
}

pub struct AmaApp {
    pub cfg: Config,
    cfg_path: PathBuf,
    base: PathBuf,
    engine: Arc<Engine>,
    shared: Arc<Shared>,
    store: Store,
    names: Vec<String>,
    preset: Preset,
    vst: BTreeMap<u32, Arc<VstPlugin>>,
    /// VSTs fora da cadeia: a thread de áudio ainda pode ter uma cópia; liberados aqui, na thread da janela.
    retired: Vec<(f64, Arc<VstPlugin>)>,
    meters: BTreeMap<u32, f32>,
    /// Preset escolhido à mão enquanto um preset por programa está ativo.
    auto_from: Option<String>,
    wizard: Option<wizard::Wizard>,
    upd: Upd,
    upd_rx: Option<std::sync::mpsc::Receiver<UpdMsg>>,
    errs: BTreeMap<u32, String>,
    cards: Cards,
    graph: Graph,
    spec: Spectrum,
    spec_tmp: Vec<f32>,
    m_in: Meter,
    m_out: Meter,
    settings: Settings,
    sys: Sys,
    hwnd: isize,
    sel: Option<u32>,
    dirty_preset: Option<f64>,
    dirty_cfg: bool,
    pending: BTreeMap<u32, f64>,
    ask_close: bool,
    remember: bool,
    dialog: Option<Dialog>,
    toast: Option<(String, f64, bool)>,
    quitting: bool,
    stopped: bool,
    scan: Option<Vec<PathBuf>>,
    last_t: f64,
}

pub struct Boot {
    pub cfg: Config,
    pub cfg_path: PathBuf,
    pub base: PathBuf,
    pub engine: Arc<Engine>,
    pub shared: Arc<Shared>,
    pub store: Store,
    pub preset: Preset,
    pub vst: BTreeMap<u32, Arc<VstPlugin>>,
    pub errs: Vec<(u32, String)>,
}

impl AmaApp {
    pub fn new(cc: &eframe::CreationContext, b: Boot) -> AmaApp {
        install(&cc.egui_ctx);
        let hwnd = sys::hwnd_of(cc).unwrap_or(0);
        sys::dark_title(hwnd);
        let mut sys = Sys::new(&cc.egui_ctx);
        sys.hwnd.store(hwnd, Relaxed);
        let names = b.store.list();
        sys.build_tray(&cc.egui_ctx, &names, &b.preset.name);
        let mut app = AmaApp {
            cfg: b.cfg,
            cfg_path: b.cfg_path,
            base: b.base,
            engine: b.engine,
            shared: b.shared,
            store: b.store,
            names,
            sel: None,
            preset: b.preset,
            vst: b.vst,
            errs: b.errs.into_iter().collect(),
            cards: Cards::default(),
            graph: Graph::new(),
            spec: Spectrum::new(),
            spec_tmp: vec![0.0; micclean::engine::SPEC_RING],
            m_in: Meter::new(),
            m_out: Meter::new(),
            settings: Settings::default(),
            sys,
            hwnd,
            dirty_preset: None,
            retired: Vec::new(),
            meters: BTreeMap::new(),
            dirty_cfg: false,
            pending: BTreeMap::new(),
            ask_close: false,
            remember: false,
            dialog: None,
            toast: None,
            quitting: false,
            stopped: false,
            scan: None,
            last_t: 0.0,
            auto_from: None,
            wizard: None,
            upd: Upd::None,
            upd_rx: None,
        };
        if app.cfg.app.check_updates {
            let (tx, rx) = std::sync::mpsc::channel();
            let c = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                match update::check() {
                    Ok(Some(r)) => {
                        log!("atualização disponível: {}", r.tag);
                        let _ = tx.send(UpdMsg::Found(r));
                    }
                    Ok(None) => {}
                    Err(e) => log!("procurar atualização: {e}"),
                }
                c.request_repaint();
            });
            app.upd_rx = Some(rx);
        }
        if app.cfg.app.setup_pending {
            app.wizard = Some(Default::default());
        }
        app.sys.focus_on.store(app.cfg.app.auto_preset, Relaxed);
        app.apply_hotkeys();
        if let Some(e) = app.settings.hotkey_err.clone() {
            app.toast = Some((tr!("Atalho: {e}", "Hotkey: {e}"), 0.0, true));
        }
        app.share_loud();
        app.sync_tray();
        app
    }

    fn st(&self) -> &micclean::stats::Stats {
        &self.engine.stats
    }

    fn note(&mut self, s: impl Into<String>, err: bool) {
        let s = s.into();
        log!("{s}");
        self.toast = Some((s, self.last_t, err));
    }

    fn builder_build(&mut self, b: &BlockCfg) -> (micclean::fx::chain::Fx, Option<String>) {
        let mut bld = Builder { vst: &mut self.vst, vol_db: self.shared.vol_db.get(), base: &self.base };
        bld.build(b)
    }

    fn share_loud(&self) {
        let v = self
            .preset
            .blocks
            .iter()
            .filter_map(|b| match b.kind {
                Kind::Loudness { ref_level, ref_offset, attenuation } => Some((b.id, ref_level, ref_offset, attenuation)),
                _ => None,
            })
            .collect();
        *self.shared.loud.lock().unwrap() = v;
    }

    fn touch(&mut self) {
        if self.dirty_preset.is_none() {
            self.dirty_preset = Some(self.last_t);
        }
    }

    /// Recria a cadeia inteira (ordem, blocos novos ou removidos).
    fn rebuild_all(&mut self) {
        let ids: Vec<u32> = self.preset.blocks.iter().map(|b| b.id).collect();
        let gone: Vec<u32> = self.vst.keys().filter(|k| !ids.contains(k)).copied().collect();
        for k in gone {
            if let Some(p) = self.vst.remove(&k) {
                self.retire(p);
            }
        }
        let mut bld = Builder { vst: &mut self.vst, vol_db: self.shared.vol_db.get(), base: &self.base };
        let (chain, errs) = build_chain(&self.preset, &mut bld);
        self.errs = errs.into_iter().collect();
        self.engine.send(Cmd::SetChain(chain));
        let aec = self.preset.blocks.iter().find_map(|b| if let Kind::Aec { tuning } = &b.kind { Some(tuning.clone()) } else { None });
        let dfn = self.preset.blocks.iter().find_map(|b| if let Kind::Denoise { atten_db, .. } = &b.kind { Some(*atten_db) } else { None });
        if let Some(t) = aec {
            self.engine.send(Cmd::AecTuning(t));
        }
        if let Some(db) = dfn {
            self.engine.send(Cmd::Atten(db));
        }
        self.pending.clear();
        self.share_loud();
        self.touch();
    }

    fn rebuild_one(&mut self, id: u32) {
        let Some(b) = self.preset.blocks.iter().find(|b| b.id == id).cloned() else { return };
        let (fx, e) = self.builder_build(&b);
        match e {
            Some(e) => {
                self.errs.insert(id, e);
            }
            None => {
                self.errs.remove(&id);
            }
        }
        self.engine.send(Cmd::Replace(id, Box::new(fx)));
    }

    fn live(&mut self, id: u32) {
        let Some(b) = self.preset.blocks.iter().find(|b| b.id == id) else { return };
        let sr = SR as f64;
        let cmd = match &b.kind {
            Kind::Aec { tuning } => Cmd::AecTuning(tuning.clone()),
            Kind::Denoise { atten_db, .. } => Cmd::Atten(*atten_db),
            Kind::Preamp { gain_db } => Cmd::Gain(id, db(*gain_db)),
            Kind::Filter { p } => Cmd::Coefs(id, Coefs::new(p, sr)),
            Kind::Loudness { ref_level, ref_offset, attenuation } => {
                Cmd::Loud(id, loudness::coefs(*ref_level, *ref_offset, *attenuation, self.shared.vol_db.get(), sr))
            }
            Kind::Limiter { ceiling_db, release_ms } => Cmd::Lim(id, *ceiling_db, *release_ms),
            Kind::Compressor { p } => Cmd::Comp(id, *p),
            Kind::DeEsser { p } => Cmd::DeEss(id, *p),
            _ => return,
        };
        self.engine.send(cmd);
        self.share_loud();
        self.touch();
    }

    fn sync_vst_state(&mut self) {
        for b in &mut self.preset.blocks {
            if let (Kind::Vst { chunk, params, .. }, Some(p)) = (&mut b.kind, self.vst.get(&b.id)) {
                let (c, ps) = p.get_state();
                *params = if c.is_some() { BTreeMap::new() } else { ps };
                *chunk = c;
            }
        }
    }

    fn save_preset(&mut self) {
        self.sync_vst_state();
        if let Err(e) = self.store.save(&self.preset) {
            self.note(tr!("não consegui salvar o preset: {e}", "could not save the preset: {e}"), true);
        }
        self.dirty_preset = None;
    }

    fn save_cfg(&mut self) {
        if let Err(e) = self.cfg.save(&self.cfg_path) {
            log!("ama.toml: {e}");
        }
        self.dirty_cfg = false;
    }

    /// Leitura suavizada do medidor por bloco.
    fn block_meters(&mut self) -> BTreeMap<u32, f32> {
        let mut m = BTreeMap::new();
        if self.st().bypass.load(Relaxed) {
            self.meters.clear();
            return m;
        }
        for (id, v) in micclean::stats::block_meters(self.st()) {
            let prev = self.meters.get(&id).copied().filter(|p| p.is_finite());
            let v = match (prev, v.is_finite()) {
                (Some(p), true) => p + 0.2 * (v - p),
                (_, true) => v,
                (p, false) => p.unwrap_or(f32::NAN),
            };
            m.insert(id, v);
        }
        self.meters = m.clone();
        m
    }

    fn retire(&mut self, p: Arc<VstPlugin>) {
        p.close_editor();
        self.retired.push((self.last_t, p));
    }

    fn retire_all(&mut self) {
        for (_, p) in std::mem::take(&mut self.vst) {
            self.retire(p);
        }
    }

    fn close_all_editors(&self) {
        for p in self.vst.values() {
            p.close_editor();
        }
    }

    fn switch_preset(&mut self, name: &str) {
        self.auto_from = None;
        self.load_preset(name, true);
    }

    /// Programa em foco mudou: entra no preset dele ou volta ao escolhido à mão.
    fn on_focus(&mut self, exe: &str) {
        let own = std::env::current_exe().ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_lowercase()));
        if !self.cfg.app.auto_preset || own.as_deref() == Some(exe) {
            return;
        }
        let rule = self.cfg.app.program_presets.iter().find(|r| r.exe.trim().eq_ignore_ascii_case(exe) && self.names.contains(&r.preset)).map(|r| r.preset.clone());
        match rule {
            Some(p) => {
                if p != self.preset.name {
                    let back = self.auto_from.clone().unwrap_or_else(|| self.preset.name.clone());
                    log!("preset por programa: {exe} -> {p}");
                    if self.load_preset(&p, false) {
                        self.auto_from = Some(back);
                    }
                }
            }
            None => self.leave_auto(),
        }
    }

    fn leave_auto(&mut self) {
        if let Some(m) = self.auto_from.take() {
            self.load_preset(&m, true);
        }
    }

    fn load_preset(&mut self, name: &str, persist: bool) -> bool {
        if name == self.preset.name {
            return true;
        }
        match self.store.load(name) {
            Ok(p) => {
                self.save_preset();
                self.close_all_editors();
                self.retire_all();
                self.preset = p;
                self.sel = None;
                self.cards.open.clear();
                self.rebuild_all();
                self.dirty_preset = None;
                if persist {
                    self.cfg.app.preset = name.to_string();
                    self.dirty_cfg = true;
                }
                self.sync_tray();
                log!("preset: {name}");
                true
            }
            Err(e) => {
                self.note(e, true);
                false
            }
        }
    }

    fn adopt_new(&mut self, p: Preset) {
        self.save_preset();
        self.close_all_editors();
        self.retire_all();
        self.preset = p;
        self.sel = None;
        self.cards.open.clear();
        self.rebuild_all();
        self.save_preset();
        self.cfg.app.preset = self.preset.name.clone();
        self.dirty_cfg = true;
        self.names = self.store.list();
        self.sync_tray();
    }

    fn sync_tray(&mut self) {
        let (m, b) = (self.st().mute.load(Relaxed), self.st().bypass.load(Relaxed));
        if let Some(t) = &self.sys.tray {
            t.mute.set_checked(m);
            t.bypass.set_checked(b);
        }
        let names = self.names.clone();
        self.sys.refresh_presets(&names, &self.preset.name);
    }

    fn apply_hotkeys(&mut self) {
        let (m, b) = (self.cfg.app.hotkey_mute.clone(), self.cfg.app.hotkey_bypass.clone());
        self.settings.hotkey_err = self.sys.set_hotkeys(&m, &b).err();
        if let Some(e) = &self.settings.hotkey_err {
            log!("atalhos: {e}");
        }
    }

    fn toggle(&mut self, which: u8) {
        let st = &self.engine.stats;
        let a = if which == 0 { &st.mute } else { &st.bypass };
        let v = !a.load(Relaxed);
        a.store(v, Relaxed);
        log!("{}: {}", if which == 0 { "mutar" } else { "bypass" }, if v { "ligado" } else { "desligado" });
        self.sync_tray();
    }

    fn insert_block(&mut self, kind: Kind) {
        let id = self.preset.next_id();
        let at = match self.preset.blocks.last() {
            Some(BlockCfg { kind: Kind::Limiter { .. }, .. }) if !matches!(kind, Kind::Limiter { .. }) => self.preset.blocks.len() - 1,
            _ => self.preset.blocks.len(),
        };
        self.preset.blocks.insert(at, BlockCfg { id, enabled: true, kind });
        self.cards.open.insert(id);
        self.sel = Some(id);
        self.rebuild_all();
    }

    fn pick_vst(&self) -> Option<PathBuf> {
        let dir = self.cfg.app.vst_dirs.iter().find(|d| Path::new(d).is_dir()).cloned().unwrap_or_default();
        rfd::FileDialog::new().set_title(t!("Plugin VST2 (64 bits)", "VST2 plugin (64-bit)")).add_filter(t!("Plugin VST", "VST plugin"), &["dll"]).set_directory(dir).pick_file()
    }

    fn handle_edits(&mut self, edits: Vec<Edit>) {
        let mut structure = false;
        for e in edits {
            match e {
                Edit::Live(id) => self.live(id),
                Edit::Rebuild(id) => {
                    self.pending.entry(id).or_insert(self.last_t);
                    self.touch();
                }
                Edit::Structure => structure = true,
                Edit::Enable(id) => {
                    let on = self.preset.blocks.iter().find(|b| b.id == id).is_some_and(|b| b.enabled);
                    let failed = self.vst.get(&id).is_some_and(|p| p.failed.load(Relaxed)) || self.errs.contains_key(&id);
                    if on && failed {
                        if let Some(p) = self.vst.remove(&id) {
                            self.retire(p);
                        }
                        self.rebuild_one(id);
                    }
                    self.engine.send(Cmd::Enable(id, on));
                    self.touch();
                }
                Edit::OpenEditor(id) => {
                    if let Some(p) = self.vst.get(&id).cloned() {
                        if p.editor_open() {
                            p.close_editor();
                        } else if let Err(e) = p.open_editor() {
                            self.note(format!("{}: {e}", p.name), true);
                        }
                    }
                }
                Edit::PickIr(id) => {
                    if let Some(f) = rfd::FileDialog::new().set_title(t!("Resposta ao impulso", "Impulse response")).add_filter("WAV", &["wav"]).pick_file() {
                        if let Some(Kind::Convolution { file }) = self.preset.blocks.iter_mut().find(|b| b.id == id).map(|b| &mut b.kind) {
                            *file = f.to_string_lossy().into_owned();
                        }
                        self.rebuild_one(id);
                        self.touch();
                    }
                }
                Edit::PickVst(id) => {
                    if let Some(f) = self.pick_vst() {
                        if let Some(Kind::Vst { library, chunk, params }) = self.preset.blocks.iter_mut().find(|b| b.id == id).map(|b| &mut b.kind) {
                            *library = f.to_string_lossy().into_owned();
                            *chunk = None;
                            params.clear();
                        }
                        if let Some(p) = self.vst.remove(&id) {
                            self.retire(p);
                        }
                        self.rebuild_one(id);
                        self.touch();
                    }
                }
                Edit::Select(id) => self.sel = Some(id),
            }
        }
        if structure {
            self.rebuild_all();
        }
    }

    fn import_eapo(&mut self) {
        let start = Path::new(r"C:\Program Files\EqualizerAPO\config");
        let Some(f) = rfd::FileDialog::new().set_title(t!("Importar configuração do Equalizer APO", "Import Equalizer APO config")).add_filter(t!("Texto", "Text"), &["txt"]).set_directory(start).pick_file() else {
            return;
        };
        match eapo::import(&f) {
            Ok((blocks, warn)) => {
                let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| t!("Importado", "Imported").into());
                let mut p = Preset::default_chain(&self.store.unique(&stem));
                let lim = p.blocks.pop();
                let n = blocks.len();
                for mut b in blocks {
                    b.id = p.next_id();
                    p.blocks.push(b);
                }
                if let Some(mut l) = lim {
                    l.id = p.next_id();
                    p.blocks.push(l);
                }
                self.adopt_new(p);
                let mut lines = vec![tr!("{n} blocos importados de {}.", "{n} blocks imported from {}.", f.display())];
                lines.extend(warn);
                if !self.errs.is_empty() {
                    lines.push(t!("Alguns blocos não carregaram e estão marcados em vermelho.", "Some blocks did not load and are marked in red.").into());
                }
                self.dialog = Some(Dialog::Info(t!("Importação concluída", "Import finished").into(), lines));
            }
            Err(e) => self.note(tr!("não consegui importar: {e}", "could not import: {e}"), true),
        }
    }

    fn export_eapo(&mut self) {
        self.sync_vst_state();
        let Some(f) = rfd::FileDialog::new()
            .set_title(t!("Exportar para o Equalizer APO", "Export to Equalizer APO"))
            .add_filter(t!("Texto", "Text"), &["txt"])
            .set_file_name(format!("{}.txt", self.preset.name))
            .save_file()
        else {
            return;
        };
        let txt = eapo::export(&self.preset.blocks, &self.base);
        match std::fs::write(&f, txt) {
            Ok(()) => self.note(tr!("exportado para {}", "exported to {}", f.display()), false),
            Err(e) => self.note(tr!("não consegui exportar: {e}", "could not export: {e}"), true),
        }
    }

    fn shutdown(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.save_preset();
        self.save_cfg();
        self.close_all_editors();
        self.shared.stop.store(true, Relaxed);
        self.engine.stop();
        self.engine.collect();
        log!("AMA encerrado");
    }

    fn start_update(&mut self, ctx: &egui::Context, r: update::Release) {
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        self.upd = Upd::Installing(r.tag.clone());
        std::thread::spawn(move || {
            let _ = tx.send(match update::install(&r) {
                Ok(p) => UpdMsg::Installed(p),
                Err(e) => UpdMsg::Failed(e),
            });
            c.request_repaint();
        });
        self.upd_rx = Some(rx);
    }

    fn poll_update(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.upd_rx else { return };
        let Ok(m) = rx.try_recv() else { return };
        self.upd_rx = None;
        match m {
            UpdMsg::Found(r) => self.upd = Upd::Available(r),
            UpdMsg::Installed(exe) => {
                self.upd = Upd::None;
                match std::process::Command::new(&exe).arg("--after-update").spawn() {
                    Ok(_) => self.quit(ctx),
                    Err(e) => self.note(tr!("Atualizado, mas não consegui abrir de novo: {e}. Abra o AMA outra vez.", "Updated, but could not restart: {e}. Open AMA again."), true),
                }
            }
            UpdMsg::Failed(e) => {
                self.upd = Upd::None;
                self.note(tr!("A atualização falhou: {e}", "The update failed: {e}"), true);
            }
        }
    }

    fn update_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut go = None;
        ui.horizontal(|ui| {
            match &self.upd {
                Upd::Available(r) => {
                    ui.label(RichText::new(tr!("AMA {} disponível", "AMA {} available", r.tag)).font(semibold(13.5)).color(TEXT));
                    label_muted(ui, tr!("(você tem a {})", "(you have {})", env!("CARGO_PKG_VERSION")));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(t!("Depois", "Later")).clicked() {
                            go = Some(false);
                        }
                        if !r.page.is_empty() {
                            ui.hyperlink_to(t!("Novidades", "What's new"), &r.page);
                        }
                        if ui.add(egui::Button::new(RichText::new(t!("Atualizar", "Update")).color(Color32::WHITE)).fill(ACCENT_DIM)).clicked() {
                            go = Some(true);
                        }
                    });
                }
                Upd::Installing(tag) => {
                    ui.spinner();
                    label_muted(ui, tr!("Baixando o AMA {tag}. Ele reabre sozinho quando terminar.", "Downloading AMA {tag}. It reopens by itself when done."));
                }
                Upd::None => {}
            }
        });
        match go {
            Some(true) => {
                if let Upd::Available(r) = std::mem::replace(&mut self.upd, Upd::None) {
                    self.start_update(&ctx, r);
                }
            }
            Some(false) => self.upd = Upd::None,
            None => {}
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        self.shutdown();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn to_tray(&mut self, ctx: &egui::Context) {
        self.save_preset();
        sys::hide_window(ctx);
    }

    // ---------- interface ----------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.label(RichText::new("AMA").font(semibold(21.0)).color(TEXT));
            ui.label(RichText::new("Another Mic App").color(FAINT).size(12.5));
            ui.add_space(18.0);
            let mut pick = None;
            egui::ComboBox::from_id_salt("preset")
                .selected_text(RichText::new(&self.preset.name).font(semibold(14.0)))
                .width(220.0)
                .show_ui(ui, |ui| {
                    for n in &self.names {
                        if ui.selectable_label(*n == self.preset.name, n).clicked() {
                            pick = Some(n.clone());
                        }
                    }
                });
            if let Some(n) = pick {
                self.switch_preset(&n);
            }
            ui.menu_button(RichText::new(t!("Preset", "Preset")).color(MUTED), |ui| {
                ui.set_min_width(230.0);
                if ui.button(t!("Novo preset", "New preset")).clicked() {
                    self.dialog = Some(Dialog::Name(NameKind::New, self.store.unique(t!("Novo preset", "New preset"))));
                }
                if ui.button(t!("Duplicar", "Duplicate")).clicked() {
                    self.dialog = Some(Dialog::Name(NameKind::Duplicate, self.store.unique(&tr!("{} cópia", "{} copy", self.preset.name))));
                }
                if ui.button(t!("Renomear", "Rename")).clicked() {
                    self.dialog = Some(Dialog::Name(NameKind::Rename, self.preset.name.clone()));
                }
                let can_rm = self.names.len() > 1;
                if ui.add_enabled(can_rm, egui::Button::new(t!("Excluir", "Delete"))).clicked() {
                    let old = self.preset.name.clone();
                    if let Some(next) = self.names.iter().find(|n| **n != old).cloned() {
                        self.switch_preset(&next);
                        match self.store.remove(&old) {
                            Ok(()) => self.note(tr!("preset '{old}' movido para presets\\_excluidos", "preset '{old}' moved to presets\\_excluidos"), false),
                            Err(e) => self.note(e, true),
                        }
                        self.names = self.store.list();
                        self.sync_tray();
                    }
                }
                ui.separator();
                if ui.button(t!("Importar do Equalizer APO", "Import from Equalizer APO")).clicked() {
                    self.import_eapo();
                }
                if ui.button(t!("Exportar para o Equalizer APO", "Export to Equalizer APO")).clicked() {
                    self.export_eapo();
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(t!("Ajustes", "Settings")).clicked() {
                    self.settings.open = !self.settings.open;
                }
                ui.add_space(6.0);
                let mon = self.st().monitor.load(Relaxed);
                if pill(ui, t!("Escuta", "Monitor"), mon, ACCENT_DIM).on_hover_text(t!("Ouvir o mic processado no dispositivo de escuta", "Hear the processed mic on the monitor device")).clicked() {
                    self.st().monitor.store(!mon, Relaxed);
                }
                let by = self.st().bypass.load(Relaxed);
                if pill(ui, "Bypass", by, Color32::from_rgb(0x9a, 0x6a, 0x22)).on_hover_text(tr!("Mic sem efeitos ({})", "Mic without effects ({})", self.cfg.app.hotkey_bypass)).clicked() {
                    self.toggle(1);
                }
                let mu = self.st().mute.load(Relaxed);
                if pill(ui, t!("Mutar", "Mute"), mu, Color32::from_rgb(0xb8, 0x34, 0x40)).on_hover_text(tr!("Silencia a saída ({})", "Silences the output ({})", self.cfg.app.hotkey_mute)).clicked() {
                    self.toggle(0);
                }
            });
        });
    }

    fn chain_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(t!("Cadeia", "Chain")).font(semibold(15.0)));
            label_muted(ui, tr!("{} blocos, de cima para baixo", "{} blocks, top to bottom", self.preset.blocks.len()));
        });
        ui.add_space(2.0);
        let avail = ui.available_height() - 46.0;
        let mut edits = vec![];
        egui::ScrollArea::vertical().max_height(avail).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let meters = self.block_meters();
            let cx = cards::Ctx { errs: &self.errs, vst: &self.vst, sel: self.sel, vol_db: self.shared.vol_db.get(), meters: &meters };
            edits = self.cards.show(ui, &mut self.preset.blocks, &cx);
            if self.preset.blocks.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| label_muted(ui, t!("Nenhum bloco. O mic passa direto.", "No blocks. The mic goes straight through.")));
            }
        });
        self.handle_edits(edits);
        ui.add_space(6.0);
        let has_aec = self.preset.blocks.iter().any(|b| matches!(b.kind, Kind::Aec { .. }));
        let has_dfn = self.preset.blocks.iter().any(|b| matches!(b.kind, Kind::Denoise { .. }));
        let mut add: Option<Kind> = None;
        let mut add_vst: Option<PathBuf> = None;
        let mut browse = false;
        ui.menu_button(RichText::new(t!("Adicionar bloco", "Add block")).font(semibold(14.0)).color(ACCENT), |ui| {
            ui.set_min_width(260.0);
            for k in 0..cards::NEW_COUNT {
                let ok = !(k == 7 && has_aec || k == 8 && has_dfn);
                if ui.add_enabled(ok, egui::Button::new(cards::new_name(k))).clicked() {
                    add = Some(cards::new_kind(k));
                }
            }
            ui.separator();
            ui.menu_button(t!("Plugin VST", "VST plugin"), |ui| {
                ui.set_min_width(300.0);
                let list = self.scan.get_or_insert_with(|| vst::scan(&self.cfg.app.vst_dirs));
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for p in list.iter() {
                        let n = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        if ui.button(n).on_hover_text(p.display().to_string()).clicked() {
                            add_vst = Some(p.clone());
                        }
                    }
                    if list.is_empty() {
                        label_muted(ui, t!("Nenhum plugin nas pastas configuradas.", "No plugins in the configured folders."));
                    }
                });
                ui.separator();
                if ui.button(t!("Procurar arquivo", "Browse for file")).clicked() {
                    browse = true;
                }
                if ui.button(t!("Atualizar lista", "Refresh list")).clicked() {
                    self.scan = None;
                }
            });
        });
        if browse {
            add_vst = self.pick_vst();
        }
        if let Some(k) = add {
            self.insert_block(k);
        }
        if let Some(p) = add_vst {
            self.insert_block(Kind::Vst { library: p.to_string_lossy().into_owned(), chunk: None, params: BTreeMap::new() });
        }
    }

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        let a = &self.cfg.app;
        let (curve, specv, meters, status) = (a.show_curve, a.show_spectrum, a.show_meters, a.show_status);
        if curve || specv {
            let h_extra = if meters { 92.0 } else { 0.0 } + if status { 178.0 } else { 0.0 };
            let h = (ui.available_height() - h_extra - 16.0).max(220.0);
            let bypass = self.st().bypass.load(Relaxed);
            let spec = specv.then_some(&self.spec);
            let edits = self.graph.show(ui, &mut self.preset.blocks, self.sel, self.shared.vol_db.get(), spec, curve, bypass, h);
            for e in edits {
                match e {
                    GraphEdit::Filter(id) => self.live(id),
                    GraphEdit::Geq(id) => self.handle_edits(vec![Edit::Rebuild(id)]),
                    GraphEdit::Select(id) => {
                        self.sel = Some(id);
                        self.cards.open.insert(id);
                    }
                }
            }
            ui.add_space(8.0);
        }
        if meters {
            card_frame(false).show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.m_in.show(ui, t!("Entrada", "Input"));
                self.m_out.show(ui, t!("Saída", "Output"));
            });
            ui.add_space(8.0);
        }
        if status {
            card_frame(false).show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.status(ui);
            });
        }
        if !(curve || specv || meters || status) {
            ui.add_space(80.0);
            ui.vertical_centered(|ui| label_muted(ui, t!("Painéis ocultos. Ative em Ajustes.", "Panels hidden. Turn them on in Settings.")));
        }
    }

    fn status(&self, ui: &mut egui::Ui) {
        let st = self.st();
        let dot = |ui: &mut egui::Ui, ok: bool, idle: bool, name: &str, dev: &str| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(10.0, 16.0), egui::Sense::hover());
                let c = if idle { FAINT } else if ok { OK } else { ERR };
                ui.painter().circle_filled(r.center(), 4.0, c);
                ui.label(RichText::new(name).size(13.0));
                let d = if dev.is_empty() { t!("padrão do Windows", "Windows default") } else { dev };
                ui.add(egui::Label::new(RichText::new(d).color(FAINT).size(12.0)).truncate());
            });
        };
        let d = &self.cfg.devices;
        let mon = st.monitor.load(Relaxed);
        ui.columns(2, |c| {
            dot(&mut c[0], st.mic_ok.load(Relaxed), false, t!("Microfone", "Microphone"), &d.mic);
            dot(&mut c[0], st.out_ok.load(Relaxed), false, t!("Saída", "Output"), &d.output);
            dot(&mut c[0], st.ref_ok.load(Relaxed), false, t!("Referência", "Reference"), &d.speakers);
            dot(&mut c[0], st.mon_ok.load(Relaxed), !mon, t!("Escuta", "Monitor"), &d.monitor);
            let kv = |ui: &mut egui::Ui, k: &str, v: String| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(k).color(MUTED).size(13.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(RichText::new(v).monospace()));
                });
            };
            let ms = st.proc_ms_avg.get();
            kv(&mut c[1], t!("Processamento", "Processing"), tr!("{ms:.2} ms de 10 ms", "{ms:.2} ms of 10 ms"));
            kv(&mut c[1], t!("Carga do motor", "Engine load"), format!("{:.0}%", st.cpu_pct.get()));
            let ad = st.aec_delay_ms.get();
            kv(&mut c[1], t!("Atraso do eco", "Echo delay"), if ad.is_finite() { format!("{ad:.0} ms") } else { "-".into() });
            let under = get(&st.out_underruns) + get(&st.ref_underruns);
            kv(&mut c[1], t!("Falhas de buffer", "Buffer underruns"), format!("{under}"));
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let mut done: Option<(NameKind, String)> = None;
        if let Some(d) = &mut self.dialog {
            let m = egui::Modal::new(egui::Id::unique("dlg")).show(ctx, |ui| {
                ui.set_width(380.0);
                match d {
                    Dialog::Name(kind, text) => {
                        let t = match kind {
                            NameKind::New => t!("Novo preset", "New preset"),
                            NameKind::Duplicate => t!("Duplicar preset", "Duplicate preset"),
                            NameKind::Rename => t!("Renomear preset", "Rename preset"),
                        };
                        ui.label(RichText::new(t).font(semibold(16.0)));
                        ui.add_space(4.0);
                        let r = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY).char_limit(60));
                        r.request_focus();
                        let name = text.trim().to_string();
                        let taken = self.names.iter().any(|n| n.eq_ignore_ascii_case(&name)) && !matches!(kind, NameKind::Rename if name.eq_ignore_ascii_case(&self.preset.name));
                        if taken {
                            ui.label(RichText::new(t!("Já existe um preset com esse nome.", "A preset with this name already exists.")).color(WARN).size(12.5));
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let ok = !name.is_empty() && !taken;
                            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if ui.add_enabled(ok, egui::Button::new(RichText::new(t!("Confirmar", "Confirm")).color(Color32::WHITE)).fill(ACCENT_DIM)).clicked() || (ok && enter) {
                                let k = std::mem::replace(kind, NameKind::New);
                                done = Some((k, name.clone()));
                            }
                            if ui.button(t!("Cancelar", "Cancel")).clicked() {
                                close = true;
                            }
                        });
                    }
                    Dialog::Info(title, lines) => {
                        ui.label(RichText::new(title.as_str()).font(semibold(16.0)));
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                            for l in lines.iter() {
                                ui.label(RichText::new(l.as_str()).size(13.0));
                            }
                        });
                        ui.add_space(6.0);
                        if ui.button("Ok").clicked() {
                            close = true;
                        }
                    }
                }
            });
            if m.should_close() {
                close = true;
            }
        }
        if let Some((k, name)) = done {
            close = true;
            match k {
                NameKind::New => self.adopt_new(Preset::default_chain(&name)),
                NameKind::Duplicate => {
                    self.sync_vst_state();
                    let mut p = self.preset.clone();
                    p.name = name;
                    self.adopt_new(p);
                }
                NameKind::Rename => {
                    let old = self.preset.name.clone();
                    self.sync_vst_state();
                    self.preset.name = name.clone();
                    if let Err(e) = self.store.rename(&old, &self.preset) {
                        self.note(e, true);
                    }
                    self.cfg.app.preset = name;
                    self.dirty_cfg = true;
                    self.names = self.store.list();
                    self.sync_tray();
                }
            }
        }
        if close {
            self.dialog = None;
        }

        if self.ask_close {
            let mut choice = None;
            egui::Modal::new(egui::Id::unique("close")).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.label(RichText::new(t!("Fechar a janela", "Close the window")).font(semibold(16.0)));
                ui.add_space(2.0);
                label_muted(ui, t!("Na bandeja o AMA continua tratando o microfone. Saindo, o Discord deixa de receber o som do CABLE.", "In the tray AMA keeps processing the mic. If you quit, Discord stops getting sound from CABLE."));
                ui.add_space(6.0);
                ui.checkbox(&mut self.remember, t!("Lembrar a escolha", "Remember my choice"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new(t!("Ir para a bandeja", "Go to the tray")).color(Color32::WHITE)).fill(ACCENT_DIM)).clicked() {
                        choice = Some(true);
                    }
                    if ui.button(t!("Sair do AMA", "Quit AMA")).clicked() {
                        choice = Some(false);
                    }
                    if ui.button(t!("Cancelar", "Cancel")).clicked() {
                        self.ask_close = false;
                    }
                });
            });
            if let Some(tray) = choice {
                self.ask_close = false;
                if self.remember {
                    self.cfg.app.on_close = if tray { "bandeja" } else { "sair" }.into();
                    self.dirty_cfg = true;
                }
                if tray { self.to_tray(ctx) } else { self.quit(ctx) }
            }
        }

        if let Some((t, at, err)) = &self.toast {
            if self.last_t - at > 4.0 {
                self.toast = None;
            } else {
                let (t, err) = (t.clone(), *err);
                egui::Area::new(egui::Id::unique("toast")).anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -18.0)).order(egui::Order::Foreground).show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(CARD_HI)
                        .stroke(Stroke::new(1.0, if err { ERR } else { LINE }))
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(14, 8))
                        .show(ui, |ui| ui.label(RichText::new(t).size(13.0)));
                });
            }
        }
    }

    fn wizard_window(&mut self, ctx: &egui::Context) {
        let Some(w) = &mut self.wizard else { return };
        let mut evs = vec![];
        let open = w.show(ctx, &self.engine.stats, &self.cfg.devices.mic, self.last_t, &mut evs);
        if !open {
            self.wizard = None;
        }
        for e in evs {
            match e {
                wizard::WizEv::Devices { mic, output } => {
                    self.cfg.devices.mic = mic;
                    if !output.is_empty() {
                        self.cfg.devices.output = output;
                    }
                    self.engine.set_devices(self.cfg.devices.clone());
                    log!("assistente: mic '{}', saída '{}'", self.cfg.devices.mic, self.cfg.devices.output);
                    self.dirty_cfg = true;
                }
                wizard::WizEv::Atten(db) => {
                    let id = self.preset.blocks.iter_mut().find_map(|b| match &mut b.kind {
                        Kind::Denoise { atten_db, .. } => {
                            *atten_db = db;
                            Some(b.id)
                        }
                        _ => None,
                    });
                    match id {
                        Some(id) => {
                            log!("assistente: atenuação do DeepFilterNet {db:.0} dB");
                            self.live(id);
                        }
                        None => self.note(t!("Este preset não tem o bloco de redução de ruído.", "This preset has no noise reduction block."), true),
                    }
                }
                wizard::WizEv::Done => {
                    self.cfg.app.setup_pending = false;
                    self.dirty_cfg = true;
                }
            }
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        for e in self.settings.show(ctx, &mut self.cfg, &self.names) {
            self.dirty_cfg = true;
            match e {
                SetEdit::Devices => {
                    self.engine.set_devices(self.cfg.devices.clone());
                    log!("dispositivos: mic '{}', saída '{}', referência '{}', escuta '{}'", self.cfg.devices.mic, self.cfg.devices.output, self.cfg.devices.speakers, self.cfg.devices.monitor);
                }
                SetEdit::Hotkeys => self.apply_hotkeys(),
                SetEdit::Autostart => sys::set_autostart(self.cfg.app.autostart),
                SetEdit::MonitorGain => self.st().monitor_gain.set(10f64.powf(self.cfg.app.monitor_gain_db / 20.0)),
                SetEdit::VstDirs => self.scan = None,
                SetEdit::Wizard => {
                    self.settings.open = false;
                    self.wizard = Some(Default::default());
                }
                SetEdit::AutoPreset => {
                    self.sys.focus_on.store(self.cfg.app.auto_preset, Relaxed);
                    if !self.cfg.app.auto_preset {
                        self.leave_auto();
                    }
                }
                SetEdit::LowLatency => {
                    self.st().low_latency.store(self.cfg.app.low_latency, Relaxed);
                }
                SetEdit::Language => {
                    i18n::set(&self.cfg.app.language);
                    self.sys.set_language();
                    self.apply_hotkeys();
                }
                SetEdit::Saved => {}
            }
        }
    }
}

impl eframe::App for AmaApp {
    fn logic(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        self.last_t = ctx.input(|i| i.time);
        let now = self.last_t;
        self.retired.retain(|(t, _)| now - *t < 2.0);
        while let Ok(ev) = self.sys.rx.try_recv() {
            match ev {
                SysEv::Show => sys::show_window(self.hwnd, ctx),
                SysEv::Quit => self.quit(ctx),
                SysEv::Mute => self.toggle(0),
                SysEv::Bypass => self.toggle(1),
                SysEv::Preset(n) => self.switch_preset(&n),
                SysEv::Focus(exe) => self.on_focus(&exe),
            }
        }
        self.poll_update(ctx);
        if self.stopped {
            return;
        }
        // plugin que falhou: desliga e avisa
        let failed: Vec<u32> = self
            .preset
            .blocks
            .iter()
            .filter(|b| b.enabled && self.vst.get(&b.id).is_some_and(|p| p.failed.load(Relaxed)))
            .map(|b| b.id)
            .collect();
        for id in failed {
            if let Some(b) = self.preset.blocks.iter_mut().find(|b| b.id == id) {
                b.enabled = false;
            }
            self.engine.send(Cmd::Enable(id, false));
            let n = self.vst.get(&id).map(|p| p.name.clone()).unwrap_or_default();
            self.note(tr!("o plugin {n} falhou e foi desligado", "plugin {n} failed and was turned off"), true);
            self.touch();
        }
        if self.vst.values().any(|p| p.changed.swap(false, Relaxed)) {
            self.touch();
        }
        let due: Vec<u32> = self.pending.iter().filter(|(_, t)| self.last_t - **t >= 0.05).map(|(k, _)| *k).collect();
        for id in due {
            self.pending.remove(&id);
            self.rebuild_one(id);
        }
        if !self.pending.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
        if self.dirty_preset.is_some_and(|t| self.last_t - t > 1.0) {
            self.save_preset();
        }
        if self.dirty_cfg {
            self.save_cfg();
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            match self.cfg.app.on_close.as_str() {
                "sair" => self.quit(ctx),
                "bandeja" => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.to_tray(ctx);
                }
                _ => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.ask_close = true;
                    self.remember = false;
                }
            }
        }
        if let Some(r) = ctx.input(|i| i.viewport().inner_rect) {
            let s = [r.width().round(), r.height().round()];
            if s[0] >= 900.0 && s[1] >= 560.0 && s != self.cfg.app.window && !ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
                self.cfg.app.window = s;
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        if !self.stopped {
            let n = {
                let mut c = self.engine.spec.lock().unwrap();
                let n = c.slots().min(self.spec_tmp.len());
                if let Ok(ch) = c.read_chunk(n) {
                    let (a, b) = ch.as_slices();
                    self.spec_tmp[..a.len()].copy_from_slice(a);
                    self.spec_tmp[a.len()..a.len() + b.len()].copy_from_slice(b);
                    ch.commit_all();
                }
                n
            };
            self.spec.push(&self.spec_tmp[n.saturating_sub(graph::NFFT)..n]);
            if self.cfg.app.show_spectrum {
                self.spec.update();
            }
            let st = &self.engine.stats;
            let (pi, po) = (take_peak(&st.in_peak), take_peak(&st.out_peak));
            self.m_in.feed(pi, self.last_t, dt);
            self.m_out.feed(po, self.last_t, dt);
        }

        if !matches!(self.upd, Upd::None) {
            egui::Panel::top("update")
                .frame(egui::Frame::new().fill(CARD_HI).inner_margin(egui::Margin { left: 18, right: 18, top: 8, bottom: 8 }))
                .show_separator_line(false)
                .show(ui, |ui| self.update_bar(ui));
        }
        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin { left: 18, right: 18, top: 12, bottom: 12 }))
            .show_separator_line(false)
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::left("chain")
            .resizable(true)
            .default_size(520.0)
            .size_range(420.0..=760.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin { left: 18, right: 12, top: 6, bottom: 14 }))
            .show(ui, |ui| self.chain_panel(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin { left: 16, right: 18, top: 14, bottom: 14 }))
            .show(ui, |ui| self.right_panel(ui));
        self.settings_window(&ctx);
        self.dialogs(&ctx);
        self.wizard_window(&ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown();
    }
}

/// Primeira execução: ama.toml herda os ajustes do micclean (config.toml), se existir.
pub fn load_config(base: &Path) -> (Config, PathBuf) {
    let path = base.join("ama.toml");
    if !path.exists() {
        let old = ["config.toml", "micclean.toml"].iter().map(|n| base.join(n)).find(|p| p.exists());
        if let Some(t) = old.and_then(|o| std::fs::read_to_string(o).ok()) {
            if let Ok(mut c) = toml::from_str::<Config>(&t) {
                c.app.log_file = "ama.log".into();
                c.app.preset.clear();
                let _ = c.save(&path);
            }
        }
    }
    let cfg = Config::load_or_create(&path).unwrap_or_else(|e| {
        let bak = base.join(format!("ama.toml.invalido-{}", presets::chrono_stamp()));
        let _ = std::fs::rename(&path, &bak);
        eprintln!("ama.toml inválido ({e}), guardado em {}", bak.display());
        Config::default()
    });
    (cfg, path)
}
