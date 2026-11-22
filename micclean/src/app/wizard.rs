//! Assistente de primeiro uso: VB-Cable, microfone, ruído do ambiente.
use super::theme::*;
use egui::{Color32, RichText};
use micclean::audio::{self, DevInfo};
use micclean::stats::Stats;
use wasapi::Direction;

pub enum WizEv {
    Devices { mic: String, output: String },
    Atten(f32),
    Done,
}

#[derive(Default)]
pub struct Wizard {
    step: u8,
    cable: Option<String>,
    mics: Vec<DevInfo>,
    mic: String,
    probe: Option<(f64, Vec<f32>)>,
    noise_db: Option<f32>,
    atten: f64,
    loaded: bool,
}

const MEASURE_S: f64 = 3.0;

/// Ambiente mais silencioso pede menos redução, que soa mais natural.
pub fn suggest_atten(noise_db: f32) -> f32 {
    match noise_db {
        n if n < -70.0 => 12.0,
        n if n < -60.0 => 18.0,
        n if n < -50.0 => 24.0,
        n if n < -40.0 => 36.0,
        _ => 100.0,
    }
}

fn primary(ui: &mut egui::Ui, t: &str) -> bool {
    ui.add(egui::Button::new(RichText::new(t).color(Color32::WHITE)).fill(ACCENT_DIM)).clicked()
}

impl Wizard {
    fn scan(&mut self, current_mic: &str) {
        self.cable = audio::list(Direction::Render).unwrap_or_default().into_iter().find(|d| d.name.to_lowercase().contains("cable input")).map(|d| d.name);
        self.mics = audio::list(Direction::Capture).unwrap_or_default().into_iter().filter(|d| !d.name.to_lowercase().contains("cable output")).collect();
        if self.mic.is_empty() {
            let m = current_mic.to_lowercase();
            let cur = self.mics.iter().find(|d| !m.is_empty() && d.name.to_lowercase().contains(&m));
            self.mic = cur.or_else(|| self.mics.iter().find(|d| d.is_default)).or(self.mics.first()).map(|d| d.name.clone()).unwrap_or_default();
        }
        self.loaded = true;
    }

    /// `false` quando o assistente fecha.
    pub fn show(&mut self, ctx: &egui::Context, st: &Stats, current_mic: &str, now: f64, out: &mut Vec<WizEv>) -> bool {
        if !self.loaded {
            self.scan(current_mic);
        }
        let mut open = true;
        let m = egui::Modal::new(egui::Id::unique("wizard")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(t!("Primeiros passos", "Getting started")).font(semibold(17.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    label_muted(ui, tr!("{} de 4", "{} of 4", self.step + 1));
                });
            });
            ui.add_space(8.0);
            match self.step {
                0 => self.step_cable(ui, current_mic),
                1 => self.step_mic(ui, out),
                2 => self.step_noise(ui, st, now, out),
                _ => {
                    ui.label(RichText::new(t!("Tudo pronto", "All set")).font(semibold(14.0)));
                    label_muted(ui, t!("No Discord, abra Configurações > Voz e vídeo e escolha CABLE Output como dispositivo de entrada. Faça o mesmo em qualquer outro programa que for usar o mic.", "In Discord, open Settings > Voice and Video and pick CABLE Output as the input device. Do the same in any other program that uses the mic."));
                    label_muted(ui, t!("Dá para abrir este assistente de novo em Ajustes.", "You can open this assistant again in Settings."));
                    ui.add_space(8.0);
                    if primary(ui, t!("Concluir", "Finish")) {
                        out.push(WizEv::Done);
                        open = false;
                    }
                }
            }
        });
        if m.should_close() {
            out.push(WizEv::Done);
            open = false;
        }
        if self.probe.is_some() {
            ctx.request_repaint();
        }
        open
    }

    fn step_cable(&mut self, ui: &mut egui::Ui, current_mic: &str) {
        ui.label(RichText::new(t!("Cabo virtual", "Virtual cable")).font(semibold(14.0)));
        match &self.cable {
            Some(n) => {
                ui.label(RichText::new(tr!("Encontrado: {n}", "Found: {n}")).color(OK));
                label_muted(ui, t!("O AMA manda a voz tratada para ele, e o Discord lê do outro lado.", "AMA sends the cleaned voice into it, and Discord reads it on the other end."));
                ui.add_space(8.0);
                if primary(ui, t!("Próximo", "Next")) {
                    self.step = 1;
                }
            }
            None => {
                ui.label(RichText::new(t!("VB-Cable não encontrado.", "VB-Cable not found.")).color(WARN));
                label_muted(ui, t!("Ele é um driver gratuito. Instale, reinicie se o instalador pedir e volte aqui.", "It is a free driver. Install it, restart if the installer asks, and come back here."));
                ui.hyperlink_to("vb-audio.com/Cable", "https://vb-audio.com/Cable/");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if primary(ui, t!("Verificar de novo", "Check again")) {
                        self.loaded = false;
                        self.scan(current_mic);
                    }
                    if ui.button(t!("Pular", "Skip")).clicked() {
                        self.step = 1;
                    }
                });
            }
        }
    }

    fn step_mic(&mut self, ui: &mut egui::Ui, out: &mut Vec<WizEv>) {
        ui.label(RichText::new(t!("Microfone", "Microphone")).font(semibold(14.0)));
        label_muted(ui, t!("Escolha o mic que você usa para falar.", "Pick the mic you talk into."));
        egui::ComboBox::from_id_salt("wiz-mic").selected_text(if self.mic.is_empty() { t!("Nenhum", "None") } else { &self.mic }).width(420.0).show_ui(ui, |ui| {
            for d in &self.mics {
                let label = if d.is_default { tr!("{}  (padrão)", "{}  (default)", d.name) } else { d.name.clone() };
                if ui.selectable_label(self.mic == d.name, label).clicked() {
                    self.mic = d.name.clone();
                }
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.mic.is_empty(), egui::Button::new(RichText::new(t!("Próximo", "Next")).color(Color32::WHITE)).fill(ACCENT_DIM)).clicked() {
                out.push(WizEv::Devices { mic: self.mic.clone(), output: self.cable.clone().unwrap_or_default() });
                self.step = 2;
            }
            if ui.button(t!("Voltar", "Back")).clicked() {
                self.step = 0;
            }
        });
    }

    fn step_noise(&mut self, ui: &mut egui::Ui, st: &Stats, now: f64, out: &mut Vec<WizEv>) {
        ui.label(RichText::new(t!("Ruído do ambiente", "Room noise")).font(semibold(14.0)));
        label_muted(ui, t!("Fique em silêncio, sem falar, e clique em Medir. Leva 3 segundos.", "Stay quiet, without talking, and click Measure. It takes 3 seconds."));
        ui.add_space(6.0);
        if let Some((t0, v)) = &mut self.probe {
            let ms = st.in_ms.get();
            if ms.is_finite() && st.mic_ok.load(std::sync::atomic::Ordering::Relaxed) {
                v.push((10.0 * (ms + 1e-12).log10()) as f32);
            }
            let p = ((now - *t0) / MEASURE_S).clamp(0.0, 1.0) as f32;
            ui.add(egui::ProgressBar::new(p).desired_width(420.0));
            if p >= 1.0 {
                let mut v = std::mem::take(v);
                self.probe = None;
                if v.is_empty() {
                    self.noise_db = None;
                } else {
                    v.sort_by(f32::total_cmp);
                    let n = v[v.len() / 2];
                    self.noise_db = Some(n);
                    self.atten = suggest_atten(n) as f64;
                }
            }
            return;
        }
        match self.noise_db {
            Some(n) => {
                ui.label(tr!("Ruído medido: {n:.0} dBFS", "Measured noise: {n:.0} dBFS"));
                let s = suggest_atten(n);
                label_muted(ui, if s >= 100.0 { tr!("Ambiente barulhento: sugestão sem limite de atenuação.", "Noisy room: suggesting no reduction limit.") } else { tr!("Sugestão: atenuação de até {s:.0} dB. Ambientes mais calmos pedem menos e a voz fica mais natural.", "Suggestion: up to {s:.0} dB of reduction. Quieter rooms need less and the voice sounds more natural.") });
                param(ui, t!("Atenuação máxima", "Max reduction"), &mut self.atten, 6.0..=100.0, " dB", false, 0);
            }
            None => {
                if self.probe.is_none() && !st.mic_ok.load(std::sync::atomic::Ordering::Relaxed) {
                    ui.label(RichText::new(t!("O microfone ainda não conectou.", "The microphone is not connected yet.")).color(WARN));
                }
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if self.noise_db.is_some() {
                if primary(ui, t!("Aplicar", "Apply")) {
                    out.push(WizEv::Atten(self.atten as f32));
                    self.step = 3;
                }
                if ui.button(t!("Medir de novo", "Measure again")).clicked() {
                    self.probe = Some((now, vec![]));
                }
            } else if primary(ui, t!("Medir", "Measure")) {
                self.probe = Some((now, vec![]));
            }
            if ui.button(t!("Pular", "Skip")).clicked() {
                self.step = 3;
            }
            if ui.button(t!("Voltar", "Back")).clicked() {
                self.step = 1;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sugestao_cresce_com_o_ruido() {
        let v: Vec<f32> = [-80.0, -65.0, -55.0, -45.0, -30.0].iter().map(|&n| super::suggest_atten(n)).collect();
        assert!(v.windows(2).all(|w| w[0] < w[1]));
    }
}
