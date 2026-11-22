use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Id, Margin, Rect, Stroke, TextStyle, Visuals};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0x12, 0x0e, 0x12);
pub const PANEL: Color32 = Color32::from_rgb(0x18, 0x13, 0x18);
pub const CARD: Color32 = Color32::from_rgb(0x1f, 0x19, 0x1f);
pub const CARD_HI: Color32 = Color32::from_rgb(0x27, 0x1f, 0x27);
pub const LINE: Color32 = Color32::from_rgb(0x33, 0x2a, 0x33);
pub const TEXT: Color32 = Color32::from_rgb(0xee, 0xe6, 0xec);
pub const MUTED: Color32 = Color32::from_rgb(0x94, 0x87, 0x92);
pub const FAINT: Color32 = Color32::from_rgb(0x5c, 0x51, 0x5b);
pub const ACCENT: Color32 = Color32::from_rgb(0xe2, 0x4a, 0x76);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x8c, 0x2b, 0x4a);
pub const OK: Color32 = Color32::from_rgb(0x6f, 0xc2, 0x9a);
pub const WARN: Color32 = Color32::from_rgb(0xe8, 0xa8, 0x4c);
pub const ERR: Color32 = Color32::from_rgb(0xf0, 0x5a, 0x5a);

pub const SEMIBOLD: &str = "semibold";

fn load_font(names: &[&str]) -> Option<Vec<u8>> {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    names.iter().find_map(|n| std::fs::read(format!(r"{dir}\Fonts\{n}")).ok())
}

pub const ANIM_TIME: f32 = 0.2;

pub fn set_animations(ctx: &egui::Context, on: bool) {
    ctx.global_style_mut(|s| {
        s.animation_time = if on { ANIM_TIME } else { 0.0 };
        s.scroll_animation = if on { Default::default() } else { egui::style::ScrollAnimation::none() };
    });
}

/// 0 -> 1 na primeira vez que `id` aparece.
pub fn appear(ctx: &egui::Context, id: Id) -> f32 {
    let a = id.with("appear");
    if ctx.data(|d| d.get_temp::<()>(id).is_none()) {
        ctx.data_mut(|d| d.insert_temp(id, ()));
        ctx.animate_bool_responsive(a, false);
    }
    ctx.animate_bool_responsive(a, true)
}

/// Esmaece a entrada do conteúdo quando `key` muda.
pub fn fade_on_change(ui: &mut egui::Ui, id: Id, key: u64) {
    let now = ui.input(|i| i.time);
    let at = ui.data_mut(|d| {
        let e = d.get_temp_mut_or_insert_with(id, || (key, f64::NEG_INFINITY));
        if e.0 != key {
            *e = (key, now);
        }
        e.1
    });
    let dur = ui.ctx().global_style().animation_time;
    if dur > 0.0 {
        let t = ((now - at) as f32 / dur).clamp(0.0, 1.0);
        if t < 1.0 {
            ui.multiply_opacity(egui::emath::easing::cubic_out(t));
            ui.ctx().request_repaint();
        }
    }
}

/// Mostra o conteúdo abrindo e fechando a altura aos poucos.
pub fn reveal<R>(ui: &mut egui::Ui, id: Id, open: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
    let t = ui.ctx().animate_bool_responsive(id, open);
    if t <= 0.0 {
        return None;
    }
    let avail = ui.available_rect_before_wrap();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(avail).layout(*ui.layout()));
    if t < 1.0 {
        let full: f32 = ui.data(|d| d.get_temp(id.with("h"))).unwrap_or(avail.height());
        let clip = Rect::from_min_size(avail.min - egui::vec2(4.0, 0.0), egui::vec2(avail.width() + 8.0, full * t));
        child.set_clip_rect(child.clip_rect().intersect(clip));
        child.multiply_opacity(t);
    }
    let r = add(&mut child);
    let m = child.min_rect();
    ui.data_mut(|d| d.insert_temp(id.with("h"), m.height()));
    ui.allocate_rect(Rect::from_min_size(m.min, egui::vec2(m.width(), m.height() * t)), egui::Sense::hover());
    Some(r)
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    if let Some(r) = load_font(&["segoeui.ttf"]) {
        fonts.font_data.insert("ui".into(), Arc::new(FontData::from_owned(r)));
        fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "ui".into());
    }
    let mut semi = vec![];
    if let Some(b) = load_font(&["seguisb.ttf", "segoeuib.ttf"]) {
        fonts.font_data.insert("semi".into(), Arc::new(FontData::from_owned(b)));
        semi.push("semi".to_string());
    }
    semi.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), semi);
    if let Some(m) = load_font(&["CascadiaMono.ttf", "consola.ttf"]) {
        fonts.font_data.insert("mono".into(), Arc::new(FontData::from_owned(m)));
        fonts.families.get_mut(&FontFamily::Monospace).unwrap().insert(0, "mono".into());
    }
    ctx.set_fonts(fonts);

    let mut st = (*ctx.global_style()).clone();
    st.text_styles = [
        (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(19.0, FontFamily::Name(SEMIBOLD.into()))),
        (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
    ]
    .into();
    st.spacing.item_spacing = egui::vec2(8.0, 8.0);
    st.spacing.button_padding = egui::vec2(12.0, 6.0);
    st.spacing.interact_size.y = 28.0;
    st.spacing.slider_width = 180.0;
    st.spacing.combo_width = 220.0;
    st.spacing.window_margin = Margin::same(18);
    st.spacing.menu_margin = Margin::same(8);
    st.spacing.scroll.bar_width = 6.0;
    st.spacing.scroll.floating = true;

    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.code_bg_color = BG;
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_shadow = egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(120) };
    v.popup_shadow = egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(110) };
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.override_text_color = Some(TEXT);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    let r = CornerRadius::same(7);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = CARD;
    w.noninteractive.weak_bg_fill = CARD;
    w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = Color32::from_rgb(0x2a, 0x22, 0x2a);
    w.inactive.weak_bg_fill = Color32::from_rgb(0x2a, 0x22, 0x2a);
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = Color32::from_rgb(0x36, 0x2b, 0x35);
    w.hovered.weak_bg_fill = Color32::from_rgb(0x36, 0x2b, 0x35);
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x4a, 0x3b, 0x48));
    w.hovered.fg_stroke = Stroke::new(1.5, TEXT);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = ACCENT_DIM;
    w.active.weak_bg_fill = ACCENT_DIM;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(1.5, TEXT);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;
    w.open = w.hovered;
    st.visuals = v;
    st.animation_time = ANIM_TIME;
    ctx.set_global_style(st);
}

/// Interruptor liga/desliga.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(34.0, 18.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let bg = lerp(Color32::from_rgb(0x3a, 0x30, 0x39), ACCENT, t);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(9), bg);
    let cx = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
    p.circle_filled(egui::pos2(cx, rect.center().y), 6.5, if *on { Color32::WHITE } else { MUTED });
    resp
}

/// Cor que acompanha o hover aos poucos.
pub fn hover_col(ui: &egui::Ui, r: &egui::Response, off: Color32, on: Color32) -> Color32 {
    lerp(off, on, ui.ctx().animate_bool_responsive(r.id.with("hover"), r.hovered() || r.dragged()))
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// Botão de estado (Mutar, Bypass, Escuta): preenchido quando ativo.
pub fn pill(ui: &mut egui::Ui, label: &str, on: bool, color: Color32) -> egui::Response {
    let t = ui.ctx().animate_bool_responsive(ui.scope_id().with(("pill", label)), on);
    let txt = egui::RichText::new(label).font(semibold(13.5)).color(lerp(TEXT, Color32::WHITE, t));
    let b = egui::Button::new(txt)
        .fill(lerp(Color32::from_rgb(0x2a, 0x22, 0x2a), color, t))
        .corner_radius(CornerRadius::same(15))
        .min_size(egui::vec2(0.0, 30.0));
    ui.add(b)
}

pub fn card_frame(hi: bool) -> egui::Frame {
    card_frame_t(if hi { 1.0 } else { 0.0 })
}

/// `t`: 0 normal, 1 selecionado.
pub fn card_frame_t(t: f32) -> egui::Frame {
    egui::Frame::new()
        .fill(lerp(CARD, CARD_HI, t))
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, lerp(LINE, Color32::from_rgb(0x45, 0x37, 0x43), t)))
        .inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 10 })
}

pub fn label_muted(ui: &mut egui::Ui, s: impl Into<String>) {
    ui.label(egui::RichText::new(s.into()).color(MUTED).size(12.5));
}

/// Linha "nome ...... valor" com slider abaixo; retorna true se mudou.
pub fn param(ui: &mut egui::Ui, name: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, unit: &str, log: bool, dec: usize) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(name).color(MUTED).size(12.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let sp = if log { *v * 0.01 } else { (range.end() - range.start()) / 400.0 };
            let dv = egui::DragValue::new(v)
                .range(range.clone())
                .speed(sp)
                .max_decimals(dec)
                .min_decimals(dec.min(1))
                .suffix(unit);
            changed |= ui.add(dv).changed();
        });
    });
    let w = ui.available_width();
    ui.spacing_mut().slider_width = w;
    let s = egui::Slider::new(v, range).logarithmic(log).show_value(false);
    changed |= ui.add(s).changed();
    changed
}
