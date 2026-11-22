use super::graph::hz;
use super::theme::*;
use egui::{Color32, Rect, RichText, Sense, Stroke, pos2, vec2};
use micclean::fx::biquad::{BqParams, BqType, Width};
use micclean::fx::chain::{BlockCfg, GeqMode, Kind};
use micclean::fx::dynamics::CompParams;
use micclean::fx::geq;
use micclean::fx::vst::VstPlugin;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

pub enum Edit {
    /// Parâmetro que vai direto para o áudio sem recriar o bloco.
    Live(u32),
    /// Recria só este bloco (GraphicEQ, Delay, Convolução).
    Rebuild(u32),
    /// Ordem, inclusão ou remoção: recria a cadeia.
    Structure,
    Enable(u32),
    OpenEditor(u32),
    PickIr(u32),
    PickVst(u32),
    Select(u32),
}

#[derive(Default)]
pub struct Cards {
    pub open: BTreeSet<u32>,
    confirm_rm: Option<(u32, f64)>,
}

pub struct Ctx<'a> {
    pub errs: &'a BTreeMap<u32, String>,
    pub vst: &'a BTreeMap<u32, Arc<VstPlugin>>,
    pub sel: Option<u32>,
    pub vol_db: f64,
    pub meters: &'a BTreeMap<u32, f32>,
}

pub fn subtitle(b: &BlockCfg, vst: &BTreeMap<u32, Arc<VstPlugin>>) -> String {
    match &b.kind {
        Kind::Aec { tuning } => if tuning == "webrtc" { t!("WebRTC padrão", "WebRTC default").into() } else { t!("AEC3 equilibrado", "AEC3 balanced").into() },
        Kind::Denoise { atten_db, .. } => tr!("atenuação até {atten_db:.0} dB", "up to {atten_db:.0} dB of reduction"),
        Kind::Preamp { gain_db } => format!("{gain_db:+.1} dB"),
        Kind::Filter { p } => {
            let g = if p.kind.has_gain() { format!("  {:+.1} dB", p.gain) } else { String::new() };
            format!("{}  {}{g}", p.kind.label(), hz(p.freq))
        }
        Kind::GraphicEq { mode, nodes } => match mode {
            GeqMode::Bands15 => t!("15 bandas", "15 bands").into(),
            GeqMode::Bands31 => t!("31 bandas", "31 bands").into(),
            GeqMode::Free => tr!("{} pontos", "{} points", nodes.len()),
        },
        Kind::Loudness { .. } => t!("segue o volume do Windows", "follows the Windows volume").into(),
        Kind::Convolution { file } => {
            if file.is_empty() { t!("nenhum arquivo", "no file").into() } else { file_stem(file) }
        }
        Kind::Delay { ms } => format!("{ms:.1} ms"),
        Kind::Limiter { ceiling_db, release_ms } => tr!("teto {ceiling_db:.1} dB, release {release_ms:.0} ms", "ceiling {ceiling_db:.1} dB, release {release_ms:.0} ms"),
        Kind::Compressor { p } => tr!("{:.0} dB, {:.1}:1, make-up {:+.1} dB", "{:.0} dB, {:.1}:1, make-up {:+.1} dB", p.threshold_db, p.ratio, p.total_makeup()),
        Kind::DeEsser { p } => tr!("acima de {}, limiar {:.0} dB", "above {}, threshold {:.0} dB", hz(p.freq_hz), p.threshold_db),
        Kind::Vst { library, .. } => vst.get(&b.id).map(|p| p.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| file_stem(library)),
    }
}

pub fn file_stem(p: &str) -> String {
    std::path::Path::new(p).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string())
}

fn grip(ui: &mut egui::Ui) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(12.0, 22.0), Sense::drag());
    let c = if resp.hovered() || resp.dragged() { TEXT } else { FAINT };
    for row in 0..3 {
        for col in 0..2 {
            ui.painter().circle_filled(pos2(r.left() + 3.0 + col as f32 * 5.0, r.center().y - 5.0 + row as f32 * 5.0), 1.3, c);
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    resp
}

fn chevron(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    let t = ui.ctx().animate_bool_responsive(resp.id, open);
    let c = if resp.hovered() { TEXT } else { MUTED };
    let ctr = r.center();
    let a = std::f32::consts::FRAC_PI_2 * t;
    let rot = |x: f32, y: f32| pos2(ctr.x + x * a.cos() - y * a.sin(), ctr.y + x * a.sin() + y * a.cos());
    ui.painter().add(egui::Shape::line(vec![rot(-2.5, -5.0), rot(2.5, 0.0), rot(-2.5, 5.0)], Stroke::new(1.6, c)));
    resp
}

fn close_x(ui: &mut egui::Ui, armed: bool) -> egui::Response {
    if armed {
        let b = egui::Button::new(RichText::new(t!("Remover", "Remove")).size(12.5).color(Color32::WHITE)).fill(ERR).corner_radius(6);
        return ui.add(b);
    }
    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    let c = if resp.hovered() { ERR } else { FAINT };
    let k = 4.0;
    let p = ui.painter();
    p.line_segment([r.center() + vec2(-k, -k), r.center() + vec2(k, k)], Stroke::new(1.5, c));
    p.line_segment([r.center() + vec2(-k, k), r.center() + vec2(k, -k)], Stroke::new(1.5, c));
    resp.on_hover_text(t!("Remover bloco", "Remove block"))
}

impl Cards {
    pub fn show(&mut self, ui: &mut egui::Ui, blocks: &mut Vec<BlockCfg>, cx: &Ctx) -> Vec<Edit> {
        let mut edits = vec![];
        let mut mv: Option<(usize, usize)> = None;
        let mut rm: Option<usize> = None;
        let now = ui.input(|i| i.time);
        if let Some((_, t)) = self.confirm_rm {
            if now - t > 3.0 {
                self.confirm_rm = None;
            }
        }
        let dragging = egui::DragAndDrop::has_payload_of_type::<usize>(ui.ctx());
        for i in 0..blocks.len() {
            let id = blocks[i].id;
            let open = self.open.contains(&id);
            let failed = cx.errs.contains_key(&id) || cx.vst.get(&id).is_some_and(|p| p.failed.load(Relaxed));
            let hi = cx.sel == Some(id);
            let mut frame = card_frame(hi);
            if failed {
                frame = frame.stroke(Stroke::new(1.0, ERR));
            }
            let fr = frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let g = grip(ui);
                    if g.dragged() {
                        g.dnd_set_drag_payload(i);
                    }
                    let b = &mut blocks[i];
                    if toggle(ui, &mut b.enabled).changed() {
                        edits.push(Edit::Enable(id));
                    }
                    ui.add_space(2.0);
                    let title_col = if b.enabled { TEXT } else { MUTED };
                    let (title, sub) = match &b.kind {
                        Kind::Vst { .. } => (subtitle(b, cx.vst), t!("Plugin VST", "VST plugin").to_string()),
                        k => (title(k).to_string(), subtitle(b, cx.vst)),
                    };
                    let head = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let armed = self.confirm_rm.is_some_and(|(x, _)| x == id);
                        if close_x(ui, armed).clicked() {
                            if armed {
                                rm = Some(i);
                                self.confirm_rm = None;
                            } else {
                                self.confirm_rm = Some((id, now));
                            }
                        }
                        let ch = chevron(ui, open);
                        if let Some(&d) = cx.meters.get(&id).filter(|d| d.is_finite() && b.enabled) {
                            let c = if d.abs() < 0.5 { FAINT } else if d > 3.0 { WARN } else { MUTED };
                            ui.label(RichText::new(format!("{d:+.1} dB")).monospace().size(11.5).color(c))
                                .on_hover_text(t!("Quanto este bloco muda o nível agora", "How much this block changes the level right now"));
                        }
                        let rest = ui.available_rect_before_wrap();
                        let area = ui.interact(rest, ui.scope_id().with(("head", id)), Sense::click());
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.label(RichText::new(title).font(semibold(14.0)).color(title_col));
                            ui.add(egui::Label::new(RichText::new(sub).color(FAINT).size(12.5)).truncate());
                        });
                        ch.clicked() || area.clicked()
                    });
                    if head.inner {
                        if open {
                            self.open.remove(&id);
                        } else {
                            self.open.insert(id);
                        }
                        edits.push(Edit::Select(id));
                    }
                });
                if let Some(e) = cx.errs.get(&id) {
                    ui.label(RichText::new(e).color(ERR).size(12.5));
                } else if cx.vst.get(&id).is_some_and(|p| p.failed.load(Relaxed)) {
                    ui.label(RichText::new(t!("O plugin gerou som inválido e foi desligado. Ligue de novo para recarregar.", "The plugin produced invalid audio and was turned off. Turn it on again to reload it.")).color(ERR).size(12.5));
                }
                let t = ui.ctx().animate_bool_responsive(ui.scope_id().with(("open", id)), open);
                if t > 0.0 {
                    ui.add_space(4.0 * t);
                    let r = ui.max_rect();
                    ui.painter().line_segment([pos2(r.left(), ui.cursor().top()), pos2(r.right(), ui.cursor().top())], Stroke::new(1.0, LINE));
                    ui.add_space(8.0 * t);
                    ui.scope(|ui| {
                        ui.set_opacity(t);
                        if let Some(e) = editor(ui, &mut blocks[i], cx) {
                            edits.push(e);
                            edits.push(Edit::Select(id));
                        }
                    });
                }
            });
            let resp = ui.interact(fr.response.rect, ui.scope_id().with(("card", id)), Sense::hover());
            if fr.response.clicked() {
                edits.push(Edit::Select(id));
            }
            if dragging {
                if let (Some(src), Some(q)) = (resp.dnd_hover_payload::<usize>(), ui.input(|i| i.pointer.hover_pos())) {
                    let before = q.y < fr.response.rect.center().y;
                    let y = if before { fr.response.rect.top() - 4.0 } else { fr.response.rect.bottom() + 4.0 };
                    if *src != i {
                        ui.painter().line_segment([pos2(fr.response.rect.left(), y), pos2(fr.response.rect.right(), y)], Stroke::new(2.0, ACCENT));
                    }
                    if let Some(src) = resp.dnd_release_payload::<usize>() {
                        let mut to = if before { i } else { i + 1 };
                        if *src < to {
                            to -= 1;
                        }
                        if *src != to {
                            mv = Some((*src, to));
                        }
                    }
                }
            }
        }
        if let Some(src) = egui::DragAndDrop::payload::<usize>(ui.ctx()) {
            if let (Some(b), Some(q)) = (blocks.get(*src), ui.input(|i| i.pointer.hover_pos())) {
                let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("dnd-ghost")));
                let g = p.layout_no_wrap(title(&b.kind).to_string(), semibold(13.0), TEXT);
                let r = Rect::from_min_size(q + vec2(14.0, -10.0), g.size() + vec2(18.0, 10.0));
                p.rect_filled(r, 8, CARD_HI);
                p.rect_stroke(r, 8, Stroke::new(1.0, ACCENT_DIM), egui::StrokeKind::Inside);
                p.galley(r.min + vec2(9.0, 5.0), g, TEXT);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
        }
        if let Some((a, b)) = mv {
            let x = blocks.remove(a);
            blocks.insert(b, x);
            edits.push(Edit::Structure);
        }
        if let Some(i) = rm {
            let b = blocks.remove(i);
            self.open.remove(&b.id);
            edits.push(Edit::Structure);
        }
        edits
    }
}

fn seg(ui: &mut egui::Ui, items: &[&str], cur: usize) -> Option<usize> {
    let mut out = None;
    egui::Frame::new().fill(BG).corner_radius(8).inner_margin(3).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.horizontal(|ui| {
            for (k, it) in items.iter().enumerate() {
                let on = k == cur;
                let b = egui::Button::new(RichText::new(*it).size(12.5).color(if on { Color32::WHITE } else { MUTED }))
                    .fill(if on { ACCENT_DIM } else { Color32::TRANSPARENT })
                    .corner_radius(6)
                    .min_size(vec2(0.0, 24.0));
                if ui.add(b).clicked() && !on {
                    out = Some(k);
                }
            }
        });
    });
    out
}

fn editor(ui: &mut egui::Ui, b: &mut BlockCfg, cx: &Ctx) -> Option<Edit> {
    let id = b.id;
    let live = |c: bool| if c { Some(Edit::Live(id)) } else { None };
    match &mut b.kind {
        Kind::Aec { tuning } => {
            label_muted(ui, t!("Remove do microfone o som que sai das suas caixas (jogo, Discord, música).", "Removes the sound of your speakers (game, Discord, music) from the mic."));
            let cur = if tuning == "webrtc" { 1 } else { 0 };
            if let Some(k) = seg(ui, &[t!("Equilibrado", "Balanced"), t!("WebRTC padrão", "WebRTC default")], cur) {
                *tuning = if k == 1 { "webrtc".into() } else { "equilibrado".into() };
                return Some(Edit::Live(id));
            }
            None
        }
        Kind::Denoise { atten_db, .. } => {
            label_muted(ui, t!("DeepFilterNet 3. Valores menores deixam um pouco do ambiente e soam mais naturais.", "DeepFilterNet 3. Lower values keep a bit of the room and sound more natural."));
            let mut v = *atten_db as f64;
            let c = param(ui, t!("Atenuação máxima", "Max reduction"), &mut v, 6.0..=100.0, " dB", false, 0);
            *atten_db = v as f32;
            live(c)
        }
        Kind::Preamp { gain_db } => live(param(ui, t!("Ganho", "Gain"), gain_db, -30.0..=30.0, " dB", false, 1)),
        Kind::Filter { p } => filter_editor(ui, p).then_some(Edit::Live(id)),
        Kind::GraphicEq { mode, nodes } => geq_editor(ui, mode, nodes).then_some(Edit::Rebuild(id)),
        Kind::Loudness { ref_level, ref_offset, attenuation } => {
            label_muted(ui, tr!("Compensa graves e agudos conforme o volume do Windows (agora {:.1} dB).", "Makes up bass and treble based on the Windows volume (now {:.1} dB).", cx.vol_db));
            let mut c = param(ui, "Reference level", ref_level, -100.0..=100.0, " dB", false, 1);
            c |= param(ui, "Reference offset", ref_offset, 0.0..=100.0, " dB", false, 1);
            c |= param(ui, "Attenuation", attenuation, 0.0..=1.0, "", false, 2);
            live(c)
        }
        Kind::Convolution { file } => {
            label_muted(ui, t!("Resposta ao impulso em .wav (o primeiro canal é usado, reamostrado para 48 kHz).", "Impulse response in .wav (first channel is used, resampled to 48 kHz)."));
            ui.horizontal(|ui| {
                let t = if file.is_empty() { t!("Nenhum arquivo", "No file").to_string() } else { file.clone() };
                let pick = ui.button(t!("Escolher arquivo", "Choose file")).clicked();
                ui.add(egui::Label::new(RichText::new(t).color(MUTED).size(12.5)).truncate());
                pick.then_some(Edit::PickIr(id))
            })
            .inner
        }
        Kind::Delay { ms } => param(ui, t!("Atraso", "Delay"), ms, 0.0..=1000.0, " ms", false, 1).then_some(Edit::Rebuild(id)),
        Kind::Limiter { ceiling_db, release_ms } => {
            let mut c = param(ui, t!("Teto", "Ceiling"), ceiling_db, -24.0..=0.0, " dB", false, 1);
            c |= param(ui, "Release", release_ms, 5.0..=1000.0, " ms", true, 0);
            live(c)
        }
        Kind::Compressor { p } => {
            label_muted(ui, t!("Deixa a voz mais constante: abaixa o que passa do limiar e o make-up devolve o volume.", "Evens out the voice: turns down what goes over the threshold and the make-up gain brings the level back."));
            let mut c = param(ui, t!("Limiar", "Threshold"), &mut p.threshold_db, -60.0..=0.0, " dB", false, 1);
            c |= param(ui, "Ratio", &mut p.ratio, 1.0..=20.0, ":1", true, 1);
            c |= param(ui, "Attack", &mut p.attack_ms, 0.1..=100.0, " ms", true, 1);
            c |= param(ui, "Release", &mut p.release_ms, 10.0..=1000.0, " ms", true, 0);
            c |= param(ui, "Knee", &mut p.knee_db, 0.0..=24.0, " dB", false, 1);
            c |= param(ui, "Make-up", &mut p.makeup_db, -12.0..=24.0, " dB", false, 1);
            let auto = CompParams { auto_makeup: true, ..*p }.total_makeup() - p.makeup_db;
            c |= ui.checkbox(&mut p.auto_makeup, RichText::new(tr!("Make-up automático (+{:.1} dB)", "Auto make-up (+{:.1} dB)", auto)).size(12.5).color(MUTED)).changed();
            live(c)
        }
        Kind::DeEsser { p } => {
            label_muted(ui, t!("Abaixa o chiado do S e do X. Ouça a escuta e desça o limiar até o S suavizar.", "Tames harsh S and X sounds. Listen on the monitor and lower the threshold until the S softens."));
            let mut c = param(ui, t!("Frequência", "Frequency"), &mut p.freq_hz, 2000.0..=12000.0, " Hz", true, 0);
            c |= param(ui, t!("Limiar", "Threshold"), &mut p.threshold_db, -60.0..=0.0, " dB", false, 1);
            c |= param(ui, "Ratio", &mut p.ratio, 1.0..=20.0, ":1", true, 1);
            c |= param(ui, t!("Redução máxima", "Max reduction"), &mut p.range_db, 0.0..=24.0, " dB", false, 1);
            c |= ui.checkbox(&mut p.split, RichText::new(t!("Só nos agudos (split)", "Highs only (split)")).size(12.5).color(MUTED)).changed();
            live(c)
        }
        Kind::Vst { library, .. } => {
            let pl = cx.vst.get(&id);
            if let Some(p) = pl {
                label_muted(ui, tr!("{} entradas, {} saídas{}", "{} inputs, {} outputs{}", p.n_in, p.n_out, if p.latency > 0 { tr!(", latência {} amostras", ", latency {} samples", p.latency) } else { String::new() }));
            }
            ui.add(egui::Label::new(RichText::new(library.as_str()).color(FAINT).size(11.5)).truncate());
            ui.horizontal(|ui| {
                let mut e = None;
                if pl.is_some_and(|p| p.has_editor) {
                    let txt = if pl.is_some_and(|p| p.editor_open()) { t!("Janela aberta", "Window open") } else { t!("Abrir janela do plugin", "Open plugin window") };
                    if ui.button(txt).clicked() {
                        e = Some(Edit::OpenEditor(id));
                    }
                } else if pl.is_some() {
                    label_muted(ui, t!("Este plugin não tem janela própria.", "This plugin has no window of its own."));
                }
                if ui.button(t!("Trocar arquivo", "Change file")).clicked() {
                    e = Some(Edit::PickVst(id));
                }
                e
            })
            .inner
        }
    }
}

fn filter_editor(ui: &mut egui::Ui, p: &mut BqParams) -> bool {
    let mut c = false;
    ui.horizontal(|ui| {
        label_muted(ui, t!("Tipo", "Type"));
        egui::ComboBox::from_id_salt("bq-type").selected_text(p.kind.label()).width(150.0).show_ui(ui, |ui| {
            for k in BqType::ALL {
                if ui.selectable_value(&mut p.kind, k, k.label()).changed() {
                    c = true;
                }
            }
        });
        if c && !p.kind.is_shelf() && p.width_kind == Width::Slope {
            p.width_kind = Width::Q;
            p.width = 0.707;
        }
    });
    c |= param(ui, t!("Frequência", "Frequency"), &mut p.freq, 10.0..=22000.0, " Hz", true, 1);
    if p.kind.has_gain() {
        c |= param(ui, t!("Ganho", "Gain"), &mut p.gain, -30.0..=30.0, " dB", false, 1);
    }
    let kinds: Vec<(Width, &str)> = if p.kind.is_shelf() {
        vec![(Width::Q, "Q"), (Width::Bw, "BW"), (Width::Slope, "Slope")]
    } else {
        vec![(Width::Q, "Q"), (Width::Bw, "BW")]
    };
    ui.horizontal(|ui| {
        label_muted(ui, t!("Largura", "Width"));
        let cur = kinds.iter().position(|k| k.0 == p.width_kind).unwrap_or(0);
        let names: Vec<&str> = kinds.iter().map(|k| k.1).collect();
        if let Some(k) = seg(ui, &names, cur) {
            p.width_kind = kinds[k].0;
            p.width = match p.width_kind {
                Width::Q => 0.707,
                Width::Bw => 1.0,
                Width::Slope => 12.0,
            };
            c = true;
        }
    });
    if p.width <= 0.0 {
        p.width = match p.width_kind {
            Width::Q => 0.707,
            Width::Bw => 1.0,
            Width::Slope => 12.0,
        };
    }
    c |= match p.width_kind {
        Width::Q => param(ui, "Q", &mut p.width, 0.1..=30.0, "", true, 3),
        Width::Bw => param(ui, t!("Largura de banda", "Bandwidth"), &mut p.width, 0.05..=5.0, " oct", true, 2),
        Width::Slope => param(ui, t!("Inclinação", "Slope"), &mut p.width, 1.0..=24.0, " dB", false, 1),
    };
    if p.kind.is_shelf() {
        c |= ui.checkbox(&mut p.corner, RichText::new(t!("Modo DCX2496 (LS e HS sem C, como no EAPO)", "DCX2496 mode (LS and HS without C, as in EAPO)")).size(12.5).color(MUTED)).changed();
    }
    c
}

fn band_label(f: f64) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        if (k - k.round()).abs() < 0.05 { format!("{}k", k.round()) } else { format!("{k:.1}k") }
    } else {
        format!("{f:.0}")
    }
}

fn faders(ui: &mut egui::Ui, nodes: &mut [(f64, f64)]) -> bool {
    const R: f64 = 18.0;
    let n = nodes.len().max(1);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 170.0), Sense::click_and_drag());
    let body = Rect::from_min_max(rect.min, rect.max - vec2(0.0, 18.0));
    let w = body.width() / n as f32;
    let p = ui.painter_at(rect);
    p.rect_filled(body, 8, BG);
    let y_of = |g: f64| body.center().y - (g / R) as f32 * (body.height() * 0.5 - 8.0);
    let g_of = |y: f32| (((body.center().y - y) / (body.height() * 0.5 - 8.0)) as f64 * R).clamp(-R, R);
    p.line_segment([pos2(body.left() + 4.0, body.center().y), pos2(body.right() - 4.0, body.center().y)], Stroke::new(1.0, LINE));
    let ptr = resp.interact_pointer_pos().or(resp.hover_pos());
    let hot = ptr.filter(|q| body.contains(*q) || resp.dragged()).map(|q| (((q.x - body.left()) / w).floor().max(0.0) as usize).min(n - 1));
    let mut c = false;
    if let (Some(k), Some(q)) = (hot, resp.interact_pointer_pos()) {
        if resp.dragged() || resp.clicked() {
            nodes[k].1 = (g_of(q.y) * 2.0).round() / 2.0;
            c = true;
        }
    }
    if resp.double_clicked() {
        if let Some(k) = hot {
            nodes[k].1 = 0.0;
            c = true;
        }
    }
    let every = if n > 20 { 2 } else { 1 };
    for (k, &(f, g)) in nodes.iter().enumerate() {
        let cx = body.left() + w * (k as f32 + 0.5);
        let bw = (w * 0.42).clamp(3.0, 10.0);
        let col = if hot == Some(k) { ACCENT } else { Color32::from_rgb(0xb8, 0x3d, 0x62) };
        let (y0, y1) = (y_of(0.0), y_of(g));
        if (y1 - y0).abs() > 0.5 {
            p.rect_filled(Rect::from_x_y_ranges(cx - bw / 2.0..=cx + bw / 2.0, y0.min(y1)..=y0.max(y1)), 2, col);
        }
        p.line_segment([pos2(cx - bw / 2.0 - 2.0, y1), pos2(cx + bw / 2.0 + 2.0, y1)], Stroke::new(2.0, TEXT));
        if k % every == 0 {
            p.text(pos2(cx, body.bottom() + 4.0), egui::Align2::CENTER_TOP, band_label(f), egui::FontId::proportional(10.5), FAINT);
        }
    }
    if let (Some(k), Some(q)) = (hot, ptr) {
        let t = format!("{}  {:+.1} dB", hz(nodes[k].0), nodes[k].1);
        let g = p.layout_no_wrap(t, egui::FontId::proportional(12.0), TEXT);
        let x = (q.x + 10.0).min(rect.right() - g.size().x - 14.0);
        let r = Rect::from_min_size(pos2(x, body.top() + 6.0), g.size() + vec2(12.0, 6.0));
        p.rect_filled(r, 5, CARD_HI);
        p.galley(r.min + vec2(6.0, 3.0), g, TEXT);
    }
    c
}

fn geq_editor(ui: &mut egui::Ui, mode: &mut GeqMode, nodes: &mut Vec<(f64, f64)>) -> bool {
    let mut c = false;
    let cur = match mode {
        GeqMode::Bands15 => 0,
        GeqMode::Bands31 => 1,
        GeqMode::Free => 2,
    };
    ui.horizontal(|ui| {
        if let Some(k) = seg(ui, &[t!("15 bandas", "15 bands"), t!("31 bandas", "31 bands"), t!("Pontos livres", "Free points")], cur) {
            let old = nodes.clone();
            let bands: &[f64] = match k {
                0 => &geq::BANDS_15,
                1 => &geq::BANDS_31,
                _ => &[],
            };
            if !bands.is_empty() {
                *nodes = bands.iter().map(|&f| (f, if old.is_empty() { 0.0 } else { (geq::gain_at(&old, f) * 2.0).round() / 2.0 })).collect();
            } else if nodes.is_empty() {
                *nodes = vec![(1000.0, 0.0)];
            }
            *mode = [GeqMode::Bands15, GeqMode::Bands31, GeqMode::Free][k];
            c = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(t!("Zerar", "Reset")).clicked() {
                for n in nodes.iter_mut() {
                    n.1 = 0.0;
                }
                c = true;
            }
        });
    });
    if *mode == GeqMode::Free {
        label_muted(ui, t!("Arraste os pontos na curva. Duplo clique na curva adiciona, botão direito remove.", "Drag the points on the curve. Double-click the curve to add one, right-click to remove."));
        let mut del = None;
        egui::Grid::new("geq-nodes").num_columns(3).spacing(vec2(10.0, 4.0)).show(ui, |ui| {
            for (k, n) in nodes.iter_mut().enumerate() {
                let sp = n.0 * 0.005;
                c |= ui.add(egui::DragValue::new(&mut n.0).range(10.0..=22000.0).speed(sp).max_decimals(1).suffix(" Hz")).changed();
                c |= ui.add(egui::DragValue::new(&mut n.1).range(-30.0..=30.0).speed(0.05).max_decimals(1).suffix(" dB")).changed();
                if ui.small_button(t!("Remover", "Remove")).clicked() {
                    del = Some(k);
                }
                ui.end_row();
            }
        });
        if let Some(k) = del {
            nodes.remove(k);
            c = true;
        }
        if ui.button(t!("Adicionar ponto", "Add point")).clicked() {
            let f = nodes.last().map(|n| (n.0 * 2.0).min(20000.0)).unwrap_or(1000.0);
            nodes.push((f, 0.0));
            c = true;
        }
        if c {
            *nodes = geq::sorted(nodes);
        }
    } else {
        c |= faders(ui, nodes);
        label_muted(ui, t!("Arraste as barras. Duplo clique volta a banda para 0 dB.", "Drag the bars. Double-click resets a band to 0 dB."));
    }
    c
}

/// Blocos novos com valores de partida.
pub fn new_kind(k: usize) -> Kind {
    match k {
        0 => Kind::Preamp { gain_db: 0.0 },
        1 => Kind::Filter { p: BqParams { kind: BqType::Peak, freq: 1000.0, gain: 0.0, width: 1.41, width_kind: Width::Q, corner: false } },
        2 => Kind::GraphicEq { mode: GeqMode::Bands15, nodes: geq::BANDS_15.iter().map(|&f| (f, 0.0)).collect() },
        3 => Kind::Loudness { ref_level: 75.0, ref_offset: 75.0, attenuation: 1.0 },
        4 => Kind::Convolution { file: String::new() },
        5 => Kind::Delay { ms: 0.0 },
        6 => Kind::Limiter { ceiling_db: -1.5, release_ms: 50.0 },
        7 => Kind::Aec { tuning: "equilibrado".into() },
        9 => Kind::Compressor { p: Default::default() },
        10 => Kind::DeEsser { p: Default::default() },
        _ => Kind::Denoise { atten_db: 100.0, post_filter: 0.0 },
    }
}

pub fn new_name(k: usize) -> &'static str {
    match k {
        0 => "Preamp",
        1 => t!("Filtro paramétrico", "Parametric filter"),
        2 => "GraphicEQ",
        3 => "Loudness correction",
        4 => t!("Convolução", "Convolution"),
        5 => "Delay",
        6 => t!("Limitador", "Limiter"),
        7 => t!("Cancelamento de eco (AEC)", "Echo cancellation (AEC)"),
        9 => "Compressor",
        10 => "De-esser",
        _ => t!("Redução de ruído (DeepFilterNet)", "Noise reduction (DeepFilterNet)"),
    }
}

pub const NEW_COUNT: usize = 11;

pub fn title(k: &Kind) -> &'static str {
    match k {
        Kind::Preamp { .. } => new_name(0),
        Kind::Filter { .. } => new_name(1),
        Kind::GraphicEq { .. } => new_name(2),
        Kind::Loudness { .. } => new_name(3),
        Kind::Convolution { .. } => new_name(4),
        Kind::Delay { .. } => new_name(5),
        Kind::Limiter { .. } => new_name(6),
        Kind::Aec { .. } => new_name(7),
        Kind::Denoise { .. } => new_name(8),
        Kind::Compressor { .. } => new_name(9),
        Kind::DeEsser { .. } => new_name(10),
        Kind::Vst { .. } => t!("Plugin VST", "VST plugin"),
    }
}
