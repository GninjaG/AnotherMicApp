use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle, Visuals};
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

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// Botão de estado (Mutar, Bypass, Escuta): preenchido quando ativo.
pub fn pill(ui: &mut egui::Ui, label: &str, on: bool, color: Color32) -> egui::Response {
    let txt = egui::RichText::new(label).font(semibold(13.5)).color(if on { Color32::WHITE } else { TEXT });
    let b = egui::Button::new(txt)
        .fill(if on { color } else { Color32::from_rgb(0x2a, 0x22, 0x2a) })
        .corner_radius(CornerRadius::same(15))
        .min_size(egui::vec2(0.0, 30.0));
    ui.add(b)
}

pub fn card_frame(hi: bool) -> egui::Frame {
    egui::Frame::new()
        .fill(if hi { CARD_HI } else { CARD })
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, if hi { Color32::from_rgb(0x45, 0x37, 0x43) } else { LINE }))
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
