use egui::emath::TSTransform;
use egui::epaint::{Shape, shape_transform::adjust_colors};
use egui::layers::PaintList;
use egui::{Color32, Id, LayerId, Order, vec2};
use std::collections::HashMap;

/// Camadas de primeiro plano que não são dropdowns (diálogos, aviso, card arrastado).
pub const NOT_POPUPS: &[&str] = &["dlg", "close", "wizard", "toast", "card-drag"];

const SLIDE: f32 = 8.0;

struct Open {
    at: f64,
    /// -1: a lista abriu para baixo do botão, 1: para cima.
    dir: f32,
    last: PaintList,
}

/// Dropdowns e menus descem esmaecendo ao abrir e fazem o inverso ao fechar.
#[derive(Default)]
pub struct PopAnim {
    open: HashMap<LayerId, Open>,
    ghosts: Vec<(PaintList, f64, f32)>,
}

fn fade(s: &mut Shape, k: f32) {
    match s {
        Shape::Text(t) => t.opacity_factor *= k,
        Shape::Vec(v) => v.iter_mut().for_each(|x| fade(x, k)),
        _ => adjust_colors(s, move |c: &mut Color32| *c = c.gamma_multiply(k)),
    }
}

impl PopAnim {
    /// Chamar no fim do frame, depois de tudo desenhado.
    pub fn run(&mut self, ctx: &egui::Context) {
        let dur = ctx.global_style().animation_time;
        if dur <= 0.0 {
            self.open.clear();
            self.ghosts.clear();
            return;
        }
        let now = ctx.input(|i| i.time);
        let skip: Vec<Id> = NOT_POPUPS.iter().map(|n| Id::unique(*n)).collect();
        let pointer = ctx.input(|i| i.pointer.interact_pos());
        let layers: Vec<LayerId> = ctx
            .memory(|m| m.areas().visible_layer_ids())
            .into_iter()
            .filter(|l| l.order == Order::Foreground && !skip.contains(&l.id))
            .filter(|l| ctx.graphics(|g| g.get(*l).is_some_and(|p| !p.is_empty())))
            .collect();

        let closed: Vec<LayerId> = self.open.keys().filter(|l| !layers.contains(l)).copied().collect();
        for l in closed {
            if let Some(o) = self.open.remove(&l) {
                self.ghosts.push((o.last, now, o.dir));
            }
        }
        for l in layers {
            let o = self.open.entry(l).or_insert_with(|| {
                let rect = egui::AreaState::load(ctx, l.id).map(|s| s.rect());
                let dir = match (rect, pointer) {
                    (Some(r), Some(p)) if r.center().y < p.y => 1.0,
                    _ => -1.0,
                };
                Open { at: now, dir, last: PaintList::default() }
            });
            let t = ((now - o.at) as f32 / dur).clamp(0.0, 1.0);
            o.last = ctx.graphics(|g| g.get(l).cloned()).unwrap_or_default();
            if t < 1.0 {
                let e = egui::emath::easing::cubic_out(t);
                let y = o.dir * SLIDE * (1.0 - e);
                ctx.graphics_mut(|g| g.entry(l).transform_after_rounding(TSTransform::from_translation(vec2(0.0, y))));
                ctx.request_repaint();
            }
        }

        let out = dur * 0.75;
        self.ghosts.retain(|g| ((now - g.1) as f32) < out);
        for (n, (list, at, dir)) in self.ghosts.iter().enumerate() {
            let k = 1.0 - ((now - at) as f32 / out).clamp(0.0, 1.0);
            let e = egui::emath::easing::cubic_in(k);
            let mut l = list.clone();
            for i in 0..l.next_idx().0 {
                l.mutate_shape(egui::layers::ShapeIdx(i), |cs| fade(&mut cs.shape, e));
            }
            l.transform_after_rounding(TSTransform::from_translation(vec2(0.0, dir * SLIDE * 0.6 * (1.0 - k))));
            ctx.graphics_mut(|g| *g.entry(LayerId::new(Order::Foreground, Id::unique(("pop-ghost", n)))) = l);
            ctx.request_repaint();
        }
    }
}
