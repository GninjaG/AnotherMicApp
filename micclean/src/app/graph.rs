use super::theme::*;
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use micclean::SR;
use micclean::fx::biquad::Coefs;
use micclean::fx::chain::{BlockCfg, GeqMode, Kind};
use micclean::fx::{geq, loudness};
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

const FMIN: f64 = 20.0;
const FMAX: f64 = 20000.0;
pub const NFFT: usize = 4096;

pub struct Spectrum {
    buf: Vec<f32>,
    pos: usize,
    fft: Arc<dyn RealToComplex<f32>>,
    win: Vec<f32>,
    inp: Vec<f32>,
    out: Vec<realfft::num_complex::Complex<f32>>,
    pub db: Vec<f32>,
}

impl Spectrum {
    pub fn new() -> Spectrum {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(NFFT);
        let win = (0..NFFT).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / NFFT as f32).cos()).collect();
        Spectrum {
            buf: vec![0.0; NFFT],
            pos: 0,
            inp: fft.make_input_vec(),
            out: fft.make_output_vec(),
            fft,
            win,
            db: vec![-120.0; NFFT / 2 + 1],
        }
    }

    pub fn push(&mut self, x: &[f32]) {
        for &v in x {
            self.buf[self.pos] = v;
            self.pos = (self.pos + 1) % NFFT;
        }
    }

    /// Atualiza o espectro suavizado (sobe rápido, desce devagar).
    pub fn update(&mut self) {
        for i in 0..NFFT {
            self.inp[i] = self.buf[(self.pos + i) % NFFT] * self.win[i];
        }
        if self.fft.process(&mut self.inp, &mut self.out).is_err() {
            return;
        }
        let norm = 2.0 / (NFFT as f32 * 0.5);
        for (d, c) in self.db.iter_mut().zip(&self.out) {
            let v = 20.0 * (c.norm() * norm + 1e-9).log10();
            *d = if v > *d { *d * 0.4 + v * 0.6 } else { *d * 0.88 + v * 0.12 };
        }
    }

    fn at(&self, f: f64) -> f32 {
        let bin = f * NFFT as f64 / SR as f64;
        let i = bin.floor() as usize;
        let t = (bin - i as f64) as f32;
        let a = self.db.get(i).copied().unwrap_or(-120.0);
        let b = self.db.get(i + 1).copied().unwrap_or(a);
        a + (b - a) * t
    }
}

pub fn block_gain(b: &BlockCfg, f: f64, vol_db: f64) -> f64 {
    let sr = SR as f64;
    match &b.kind {
        Kind::Preamp { gain_db } => *gain_db,
        Kind::Filter { p } => Coefs::new(p, sr).gain_at(f, sr),
        Kind::GraphicEq { nodes, .. } => geq::gain_at(nodes, f),
        Kind::Loudness { ref_level, ref_offset, attenuation } => {
            let c = loudness::coefs(*ref_level, *ref_offset, *attenuation, vol_db, sr);
            if c.neutral { 0.0 } else { c.ls.gain_at(f, sr) + c.hs.gain_at(f, sr) + 20.0 * c.att.log10() }
        }
        _ => 0.0,
    }
}

fn shapes_response(b: &BlockCfg) -> bool {
    matches!(b.kind, Kind::Preamp { .. } | Kind::Filter { .. } | Kind::GraphicEq { .. } | Kind::Loudness { .. })
}

pub struct Graph {
    pub drag: Option<(u32, usize)>,
}

pub enum GraphEdit {
    /// Filtro mudou (parâmetro vivo).
    Filter(u32),
    /// Nós do GraphicEQ mudaram.
    Geq(u32),
    Select(u32),
}

struct Map {
    r: Rect,
    range: f64,
}

impl Map {
    fn x(&self, f: f64) -> f32 {
        self.r.left() + self.r.width() * ((f / FMIN).log10() / (FMAX / FMIN).log10()) as f32
    }
    fn f(&self, x: f32) -> f64 {
        FMIN * (FMAX / FMIN).powf(((x - self.r.left()) / self.r.width()).clamp(0.0, 1.0) as f64)
    }
    fn y(&self, g: f64) -> f32 {
        self.r.center().y - (g / self.range) as f32 * self.r.height() * 0.5
    }
    fn g(&self, y: f32) -> f64 {
        ((self.r.center().y - y) / (self.r.height() * 0.5)) as f64 * self.range
    }
}

impl Graph {
    pub fn new() -> Graph {
        Graph { drag: None }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        blocks: &mut [BlockCfg],
        sel: Option<u32>,
        vol_db: f64,
        spec: Option<&Spectrum>,
        curve: bool,
        bypass: bool,
        height: f32,
    ) -> Vec<GraphEdit> {
        let mut edits = vec![];
        let (outer, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
        let p = ui.painter_at(outer);
        p.rect_filled(outer, 10, BG);
        p.rect_stroke(outer, 10, Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
        let r = Rect::from_min_max(outer.min + vec2(42.0, 14.0), outer.max - vec2(14.0, 26.0));

        let n = (r.width() as usize / 2).max(64);
        let freqs: Vec<f64> = (0..=n).map(|i| FMIN * (FMAX / FMIN).powf(i as f64 / n as f64)).collect();
        let live: Vec<&BlockCfg> = blocks.iter().filter(|b| b.enabled && shapes_response(b)).collect();
        let total: Vec<f64> = freqs.iter().map(|&f| if bypass { 0.0 } else { live.iter().map(|b| block_gain(b, f, vol_db)).sum() }).collect();
        let peak = total.iter().fold(0.0f64, |m, g| m.max(g.abs()));
        let range = [12.0, 18.0, 24.0, 36.0, 48.0].into_iter().find(|&v| peak <= v - 1.0).unwrap_or(60.0);
        let m = Map { r, range };

        // grade
        let grid = Color32::from_rgb(0x24, 0x1d, 0x24);
        let font = egui::FontId::proportional(11.0);
        for f in [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0] {
            let x = m.x(f);
            p.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], Stroke::new(1.0, grid));
            let t = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
            p.text(pos2(x, r.bottom() + 6.0), egui::Align2::CENTER_TOP, t, font.clone(), FAINT);
        }
        let step = range / 3.0;
        for k in -3..=3 {
            let g = k as f64 * step;
            let y = m.y(g);
            let c = if k == 0 { Color32::from_rgb(0x3a, 0x2f, 0x39) } else { grid };
            p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], Stroke::new(1.0, c));
            p.text(pos2(r.left() - 8.0, y), egui::Align2::RIGHT_CENTER, format!("{g:+.0}"), font.clone(), FAINT);
        }

        // espectro da saída (-90..0 dBFS ocupa a altura toda)
        if let Some(s) = spec.filter(|s| s.db.iter().any(|&d| d > -100.0)) {
            let pts: Vec<Pos2> = freqs
                .iter()
                .map(|&f| {
                    let d = (s.at(f) + 3.0 * (f / 1000.0).log2() as f32).clamp(-90.0, 0.0);
                    pos2(m.x(f), r.bottom() - (d + 90.0) / 90.0 * r.height())
                })
                .collect();
            let fill = Color32::from_rgba_unmultiplied(0xe2, 0x4a, 0x76, 18);
            for w in pts.windows(2) {
                p.add(Shape::convex_polygon(
                    vec![w[0], w[1], pos2(w[1].x, r.bottom()), pos2(w[0].x, r.bottom())],
                    fill,
                    Stroke::NONE,
                ));
            }
            p.add(Shape::line(pts, Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xe2, 0x4a, 0x76, 70))));
        }

        if !curve {
            return edits;
        }

        // curva do bloco selecionado
        if let Some(b) = sel.and_then(|id| blocks.iter().find(|b| b.id == id)).filter(|b| shapes_response(b)) {
            let pts: Vec<Pos2> = freqs.iter().map(|&f| pos2(m.x(f), m.y(block_gain(b, f, vol_db)))).collect();
            p.add(Shape::line(pts, Stroke::new(1.2, Color32::from_rgba_unmultiplied(0xee, 0xe6, 0xec, 60))));
        }
        let pts: Vec<Pos2> = freqs.iter().zip(&total).map(|(&f, &g)| pos2(m.x(f), m.y(g))).collect();
        p.add(Shape::line(pts, Stroke::new(2.0, if bypass { MUTED } else { ACCENT })));

        // pontos arrastáveis
        let ptr = resp.interact_pointer_pos().or(resp.hover_pos());
        let mut handles: Vec<(u32, usize, Pos2, bool)> = vec![];
        for b in blocks.iter() {
            match &b.kind {
                Kind::Filter { p: fp } => {
                    let g = if fp.kind.has_gain() { fp.gain } else { 0.0 };
                    handles.push((b.id, 0, pos2(m.x(fp.freq), m.y(g)), b.enabled));
                }
                Kind::GraphicEq { nodes, .. } if sel == Some(b.id) => {
                    for (i, &(f, g)) in nodes.iter().enumerate() {
                        if (FMIN..=FMAX).contains(&f) {
                            handles.push((b.id, i, pos2(m.x(f), m.y(g)), b.enabled));
                        }
                    }
                }
                _ => {}
            }
        }
        let near = ptr.and_then(|q| {
            handles.iter().filter(|h| h.2.distance(q) < 11.0).min_by(|a, b| a.2.distance(q).total_cmp(&b.2.distance(q))).map(|h| (h.0, h.1))
        });
        if resp.drag_started() {
            self.drag = near;
            if let Some((id, _)) = near {
                edits.push(GraphEdit::Select(id));
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
        }
        if let (Some((id, i)), Some(q)) = (self.drag, resp.interact_pointer_pos()) {
            if let Some(b) = blocks.iter_mut().find(|b| b.id == id) {
                let f = m.f(q.x).clamp(FMIN, FMAX);
                let g = m.g(q.y).clamp(-range, range);
                match &mut b.kind {
                    Kind::Filter { p: fp } => {
                        fp.freq = (f * 10.0).round() / 10.0;
                        if fp.kind.has_gain() {
                            fp.gain = (g * 10.0).round() / 10.0;
                        }
                        edits.push(GraphEdit::Filter(id));
                    }
                    Kind::GraphicEq { mode, nodes } => {
                        if let Some(n) = nodes.get_mut(i) {
                            if *mode == GeqMode::Free {
                                n.0 = (f * 10.0).round() / 10.0;
                            }
                            n.1 = (g * 10.0).round() / 10.0;
                            edits.push(GraphEdit::Geq(id));
                        }
                    }
                    _ => {}
                }
            }
        }
        // roda do mouse ajusta a largura do filtro sob o cursor
        if let Some((id, _)) = near {
            let scroll = ui.input(|i| i.smooth_scroll_delta().y);
            if scroll != 0.0 {
                if let Some(Kind::Filter { p: fp }) = blocks.iter_mut().find(|b| b.id == id).map(|b| &mut b.kind) {
                    let w = if fp.width > 0.0 { fp.width } else { 0.707 };
                    fp.width = (w * if scroll > 0.0 { 1.1 } else { 1.0 / 1.1 }).clamp(0.05, 40.0);
                    fp.width = (fp.width * 1000.0).round() / 1000.0;
                    edits.push(GraphEdit::Filter(id));
                }
            }
        }
        // duplo clique no modo livre adiciona nó; botão direito remove
        if let Some(id) = sel {
            if let Some(b) = blocks.iter_mut().find(|b| b.id == id) {
                if let Kind::GraphicEq { mode: GeqMode::Free, nodes } = &mut b.kind {
                    if resp.double_clicked() {
                        if let Some(q) = resp.interact_pointer_pos() {
                            nodes.push(((m.f(q.x) * 10.0).round() / 10.0, (m.g(q.y) * 10.0).round() / 10.0));
                            *nodes = geq::sorted(nodes);
                            edits.push(GraphEdit::Geq(id));
                        }
                    }
                    if resp.secondary_clicked() {
                        if let Some((hid, i)) = near.filter(|h| h.0 == id) {
                            let _ = hid;
                            if nodes.len() > 1 {
                                nodes.remove(i);
                                edits.push(GraphEdit::Geq(id));
                            }
                        }
                    }
                }
            }
        }
        for (id, i, c, on) in &handles {
            let hot = near == Some((*id, *i)) || self.drag == Some((*id, *i));
            let is_sel = sel == Some(*id);
            let col = if !on { FAINT } else if is_sel || hot { ACCENT } else { TEXT };
            p.circle_filled(*c, if hot { 6.5 } else { 5.0 }, BG);
            p.circle_stroke(*c, if hot { 6.5 } else { 5.0 }, Stroke::new(2.0, col));
        }
        if let (Some((id, i)), Some(q)) = (near.or(self.drag), ptr) {
            if let Some(b) = blocks.iter().find(|b| b.id == id) {
                let t = match &b.kind {
                    Kind::Filter { p: fp } => {
                        let w = match fp.width_kind {
                            micclean::fx::biquad::Width::Q => "Q",
                            micclean::fx::biquad::Width::Bw => "BW",
                            micclean::fx::biquad::Width::Slope => "S",
                        };
                        let g = if fp.kind.has_gain() { format!("  {:+.1} dB", fp.gain) } else { String::new() };
                        format!("{}  {}{g}  {w} {:.2}", fp.kind.label(), hz(fp.freq), fp.width)
                    }
                    Kind::GraphicEq { nodes, .. } => nodes.get(i).map(|n| format!("{}  {:+.1} dB", hz(n.0), n.1)).unwrap_or_default(),
                    _ => String::new(),
                };
                let at = pos2((q.x + 12.0).min(r.right() - 150.0), (q.y - 26.0).max(r.top()));
                let g = p.layout_no_wrap(t, egui::FontId::proportional(12.0), TEXT);
                let bg = Rect::from_min_size(at, g.size() + vec2(14.0, 8.0));
                p.rect_filled(bg, 6, CARD_HI);
                p.galley(at + vec2(7.0, 4.0), g, TEXT);
            }
        }
        if near.is_some() || self.drag.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        edits
    }
}

pub fn hz(f: f64) -> String {
    if f >= 1000.0 { format!("{:.2} kHz", f / 1000.0).replace(".00 ", " ") } else { format!("{f:.0} Hz") }
}

pub struct Meter {
    level: f32,
    hold: f32,
    hold_t: f64,
}

impl Meter {
    pub fn new() -> Meter {
        Meter { level: -90.0, hold: -90.0, hold_t: 0.0 }
    }

    pub fn feed(&mut self, peak: f32, now: f64, dt: f32) {
        let db = 20.0 * (peak + 1e-9).log10();
        self.level = if db > self.level { db } else { (self.level - 30.0 * dt).max(db) };
        if db >= self.hold || now - self.hold_t > 1.5 {
            self.hold = db;
            self.hold_t = now;
        }
    }

    pub fn show(&self, ui: &mut egui::Ui, label: &str) {
        ui.horizontal(|ui| {
            ui.add_sized(vec2(56.0, 18.0), egui::Label::new(egui::RichText::new(label).color(MUTED).size(12.5)));
            let w = ui.available_width() - 64.0;
            let (r, _) = ui.allocate_exact_size(vec2(w, 10.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(r, 5, BG);
            let t = |db: f32| ((db + 60.0) / 60.0).clamp(0.0, 1.0);
            let x = r.left() + r.width() * t(self.level);
            let col = if self.level > -1.0 { ERR } else if self.level > -9.0 { WARN } else { ACCENT };
            if x > r.left() + 1.0 {
                p.rect_filled(Rect::from_min_max(r.min, pos2(x, r.bottom())), 5, col);
            }
            let hx = r.left() + r.width() * t(self.hold);
            if self.hold > -59.0 {
                p.line_segment([pos2(hx, r.top() - 2.0), pos2(hx, r.bottom() + 2.0)], Stroke::new(1.5, TEXT));
            }
            for db in [-48.0, -36.0, -24.0, -12.0, -6.0] {
                let gx = r.left() + r.width() * t(db);
                p.line_segment([pos2(gx, r.top()), pos2(gx, r.bottom())], Stroke::new(1.0, Color32::from_black_alpha(90)));
            }
            let txt = if self.hold <= -89.0 { "-inf".to_string() } else { format!("{:.1} dB", self.hold) };
            ui.add_sized(vec2(56.0, 18.0), egui::Label::new(egui::RichText::new(txt).monospace().color(TEXT).size(12.0)));
        });
    }
}
