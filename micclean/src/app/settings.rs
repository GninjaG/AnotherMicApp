use super::theme::*;
use egui::{RichText, vec2};
use micclean::audio::{self, DevInfo};
use micclean::config::Config;
use wasapi::Direction;

#[derive(Default)]
pub struct Settings {
    pub open: bool,
    caps: Vec<DevInfo>,
    rends: Vec<DevInfo>,
    loaded: bool,
    capture: Option<usize>,
    seen: u64,
    programs: Vec<String>,
    pub hotkey_err: Option<String>,
    lat: super::cable::CableUi,
}

pub enum SetEdit {
    Devices,
    Hotkeys,
    Autostart,
    MonitorGain,
    VstDirs,
    Language,
    LowLatency,
    AutoPreset,
    Wizard,
    Saved,
}

fn dev_combo(ui: &mut egui::Ui, id: &str, cur: &mut String, list: &[DevInfo], allow_default: bool) -> bool {
    let mut c = false;
    let shown = if cur.is_empty() {
        t!("Padrão do Windows", "Windows default").to_string()
    } else {
        list.iter().find(|d| audio::name_matches(&d.name, cur)).map(|d| d.name.clone()).unwrap_or_else(|| tr!("{cur} (não encontrado)", "{cur} (not found)"))
    };
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(ui.available_width().min(420.0)).show_ui(ui, |ui| {
        if allow_default && ui.selectable_label(cur.is_empty(), t!("Padrão do Windows", "Windows default")).clicked() {
            cur.clear();
            c = true;
        }
        for d in list {
            let on = audio::name_matches(&d.name, cur);
            let label = if d.is_default { tr!("{}  (padrão)", "{}  (default)", d.name) } else { d.name.clone() };
            if ui.selectable_label(on, label).clicked() {
                *cur = d.name.clone();
                c = true;
            }
        }
    });
    c
}

fn section(ui: &mut egui::Ui, t: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(t).font(semibold(13.0)).color(ACCENT));
    ui.add_space(2.0);
}

fn row(ui: &mut egui::Ui, name: &str, add: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(170.0, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(170.0);
            ui.label(RichText::new(name).color(MUTED));
        });
        add(ui);
    })
    .response
}

impl Settings {
    fn program_rules(&mut self, ui: &mut egui::Ui, cfg: &mut Config, presets: &[String]) -> bool {
        let mut c = false;
        label_muted(ui, t!("Ao sair do programa, volta para o preset que você escolheu.", "When you leave the program, it goes back to the preset you picked."));
        let mut del = None;
        for (k, r) in cfg.app.program_presets.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                c |= ui.add(egui::TextEdit::singleline(&mut r.exe).desired_width(200.0).hint_text("jogo.exe")).changed();
                ui.label(RichText::new("\u{2192}").color(MUTED));
                let shown = if r.preset.is_empty() { t!("Escolha o preset", "Pick a preset").to_string() } else { r.preset.clone() };
                egui::ComboBox::from_id_salt(("prog-preset", k)).selected_text(shown).width(200.0).show_ui(ui, |ui| {
                    for n in presets {
                        if ui.selectable_label(r.preset == *n, n).clicked() {
                            r.preset = n.clone();
                            c = true;
                        }
                    }
                });
                if !r.preset.is_empty() && !presets.contains(&r.preset) {
                    label_muted(ui, t!("preset não existe", "preset missing"));
                }
                if ui.small_button(t!("Remover", "Remove")).clicked() {
                    del = Some(k);
                }
            });
        }
        if let Some(k) = del {
            cfg.app.program_presets.remove(k);
            c = true;
        }
        let r = egui::ComboBox::from_id_salt("prog-add").selected_text(t!("Adicionar programa aberto", "Add an open program")).width(260.0).show_ui(ui, |ui| {
            let mut pick = None;
            for p in &self.programs {
                if ui.selectable_label(false, p).clicked() {
                    pick = Some(p.clone());
                }
            }
            if self.programs.is_empty() {
                label_muted(ui, t!("Nenhum programa com janela aberta", "No program with an open window"));
            }
            pick
        });
        if r.response.clicked() {
            self.programs = super::sys::window_programs();
        }
        if let Some(Some(exe)) = r.inner {
            let preset = presets.first().cloned().unwrap_or_default();
            cfg.app.program_presets.push(micclean::config::ProgramPreset { exe, preset });
            c = true;
        }
        c
    }

    fn refresh(&mut self) {
        self.caps = audio::list(Direction::Capture).unwrap_or_default();
        self.rends = audio::list(Direction::Render).unwrap_or_default();
        self.loaded = true;
        self.seen = audio::hotplug();
    }

    pub fn show(&mut self, ctx: &egui::Context, cfg: &mut Config, presets: &[String]) -> Vec<SetEdit> {
        let mut out = vec![];
        if !self.open {
            self.capture = None;
            return out;
        }
        if !self.loaded || self.seen != audio::hotplug() {
            self.refresh();
        }
        if let Some(k) = self.capture {
            let got = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            if let Some((key, m)) = got {
                if key == egui::Key::Escape {
                    self.capture = None;
                } else if let Some(s) = super::sys::format_hotkey(m, key) {
                    if k == 0 { cfg.app.hotkey_mute = s } else { cfg.app.hotkey_bypass = s }
                    self.capture = None;
                    out.push(SetEdit::Hotkeys);
                }
            }
        }
        let mut open = self.open;
        egui::Window::new(RichText::new(t!("Ajustes", "Settings")).font(semibold(16.0)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(640.0)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let h = (ctx.content_rect().height() - 150.0).max(300.0);
                egui::ScrollArea::vertical().max_height(h).min_scrolled_height(h).show(ui, |ui| {
                    ui.set_width(600.0);
                    section(ui, t!("Dispositivos", "Devices"));
                    let d = &mut cfg.devices;
                    let mut dc = false;
                    row(ui, t!("Microfone", "Microphone"), |ui| dc |= dev_combo(ui, "dev-mic", &mut d.mic, &self.caps, true));
                    row(ui, t!("Saída (para os programas)", "Output (for apps)"), |ui| dc |= dev_combo(ui, "dev-out", &mut d.output, &self.rends, false))
                        .on_hover_text(t!("Escolha CABLE Input. Nos programas (Discord, jogos), use CABLE Output como microfone.", "Pick CABLE Input. In apps (Discord, games), use CABLE Output as the microphone."));
                    row(ui, t!("Referência do eco", "Echo reference"), |ui| dc |= dev_combo(ui, "dev-ref", &mut d.speakers, &self.rends, true))
                        .on_hover_text(t!("Onde você ouve o jogo e a call. O AMA tira esse som do mic.", "Where you hear the game and the call. AMA removes that sound from the mic."));
                    row(ui, t!("Escuta", "Monitor"), |ui| dc |= dev_combo(ui, "dev-mon", &mut d.monitor, &self.rends, true))
                        .on_hover_text(t!("Onde você se ouve com o botão Escuta ligado.", "Where you hear yourself when Monitor is on."));
                    let mut g = cfg.app.monitor_gain_db;
                    row(ui, t!("Volume da escuta", "Monitor volume"), |ui| {
                        ui.spacing_mut().slider_width = 300.0;
                        if ui.add(egui::Slider::new(&mut g, -40.0..=12.0).suffix(" dB").max_decimals(1)).changed() {
                            out.push(SetEdit::MonitorGain);
                        }
                    });
                    cfg.app.monitor_gain_db = g;
                    let d = &mut cfg.devices;
                    row(ui, t!("Efeitos do Windows", "Windows effects"), |ui| {
                        dc |= ui.checkbox(&mut d.mic_raw, t!("Ignorar (modo RAW)", "Skip them (RAW mode)")).changed();
                    })
                    .on_hover_text(t!("Desliga realces do driver do mic, que costumam brigar com o AMA.", "Turns off the mic driver's enhancements, which tend to fight with AMA."));
                    if dc {
                        out.push(SetEdit::Devices);
                    }

                    section(ui, t!("Latência", "Latency"));
                    row(ui, t!("Modo rápido", "Fast mode"), |ui| {
                        if ui.checkbox(&mut cfg.app.low_latency, t!("Ligado", "On")).changed() {
                            out.push(SetEdit::LowLatency);
                        }
                    })
                    .on_hover_text(t!("DeepFilterNet sem lookahead: uns 10 ms a menos, limpa um pouco pior e usa mais CPU.", "DeepFilterNet without lookahead: about 10 ms less, slightly worse cleanup and more CPU."));
                    row(ui, t!("Saída exclusiva", "Exclusive output"), |ui| {
                        if ui.checkbox(&mut cfg.devices.out_exclusive, t!("Ligado", "On")).changed() {
                            out.push(SetEdit::Devices);
                        }
                    })
                    .on_hover_text(t!("O AMA escreve no cabo sem passar pelo mixer do Windows: uns 10 a 15 ms a menos. Nenhum outro programa consegue tocar no CABLE Input enquanto estiver ligado. Experimental: no VB-Cable 3.5 o atraso às vezes vai crescendo.", "AMA writes to the cable without the Windows mixer: about 10 to 15 ms less. No other program can play into CABLE Input while it is on. Experimental: on VB-Cable 3.5 the delay sometimes keeps growing."));
                    row(ui, t!("Buffer do VB-Cable", "VB-Cable buffer"), |ui| {
                        ui.vertical(|ui| {
                            if !self.lat.show(ui) {
                                label_muted(ui, t!("VB-Cable não instalado", "VB-Cable not installed"));
                            }
                        });
                    })
                    .on_hover_text(t!("Quanto o VB-Cable segura antes de entregar. Menor = voz chega antes.", "How much VB-Cable holds before delivering. Lower = the voice arrives sooner."));

                    section(ui, t!("Atalhos globais", "Global hotkeys"));
                    for (k, name) in [(0usize, t!("Mutar", "Mute")), (1, "Bypass")] {
                        let cur = if k == 0 { cfg.app.hotkey_mute.clone() } else { cfg.app.hotkey_bypass.clone() };
                        row(ui, name, |ui| {
                            let capturing = self.capture == Some(k);
                            let t = if capturing { t!("Pressione o atalho (Esc cancela)", "Press the hotkey (Esc cancels)").to_string() } else if cur.is_empty() { t!("Nenhum", "None").into() } else { cur.clone() };
                            let b = egui::Button::new(RichText::new(t).monospace()).min_size(vec2(220.0, 26.0)).fill(if capturing { ACCENT_DIM } else { BG });
                            if ui.add(b).clicked() {
                                self.capture = Some(k);
                            }
                            if !cur.is_empty() && ui.small_button(t!("Limpar", "Clear")).clicked() {
                                if k == 0 { cfg.app.hotkey_mute.clear() } else { cfg.app.hotkey_bypass.clear() }
                                out.push(SetEdit::Hotkeys);
                            }
                        });
                    }
                    if let Some(e) = &self.hotkey_err {
                        ui.label(RichText::new(e).color(ERR).size(12.5));
                    }

                    section(ui, t!("Sistema", "System"));
                    row(ui, t!("Ao ligar o PC", "On PC start"), |ui| {
                        if ui.checkbox(&mut cfg.app.autostart, t!("Abrir o AMA na bandeja", "Open AMA in the tray")).changed() {
                            out.push(SetEdit::Autostart);
                        }
                    });
                    row(ui, t!("Ao fechar a janela", "When closing the window"), |ui| {
                        for (v, t) in [("perguntar", t!("Perguntar", "Ask")), ("bandeja", t!("Ir para a bandeja", "Go to the tray")), ("sair", t!("Sair do AMA", "Quit AMA"))] {
                            if ui.radio(cfg.app.on_close == v, t).clicked() {
                                cfg.app.on_close = v.into();
                                out.push(SetEdit::Saved);
                            }
                        }
                    });
                    row(ui, t!("Idioma", "Language"), |ui| {
                        for (v, name) in [("pt", "Português"), ("en", "English")] {
                            if ui.radio(cfg.app.language == v, name).clicked() && cfg.app.language != v {
                                cfg.app.language = v.into();
                                out.push(SetEdit::Language);
                            }
                        }
                    });
                    row(ui, t!("Atualizações", "Updates"), |ui| {
                        if ui.checkbox(&mut cfg.app.check_updates, t!("Procurar ao abrir", "Check on start")).changed() {
                            out.push(SetEdit::Saved);
                        }
                    })
                    .on_hover_text(t!("Só instala se você aceitar.", "Only installs if you accept."));

                    section(ui, t!("Presets por programa", "Per-program presets"));
                    row(ui, t!("Troca automática", "Auto switch"), |ui| {
                        if ui.checkbox(&mut cfg.app.auto_preset, t!("Conforme o programa em foco", "Based on the focused program")).changed() {
                            out.push(SetEdit::AutoPreset);
                        }
                    });
                    if cfg.app.auto_preset && self.program_rules(ui, cfg, presets) {
                        out.push(SetEdit::Saved);
                    }

                    section(ui, t!("Painéis", "Panels"));
                    let a = &mut cfg.app;
                    let mut pc = false;
                    row(ui, t!("Mostrar", "Show"), |ui| {
                        pc |= ui.checkbox(&mut a.show_curve, t!("Curva do EQ", "EQ curve")).changed();
                        pc |= ui.checkbox(&mut a.show_spectrum, t!("Espectro", "Spectrum")).changed();
                        pc |= ui.checkbox(&mut a.show_meters, t!("Medidores", "Meters")).changed();
                        pc |= ui.checkbox(&mut a.show_status, t!("Status do motor", "Engine status")).changed();
                    });
                    if pc {
                        out.push(SetEdit::Saved);
                    }

                    section(ui, t!("Pastas de plugins VST", "VST plugin folders"));
                    let mut del = None;
                    for (k, dir) in cfg.app.vst_dirs.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let exists = std::path::Path::new(dir).is_dir();
                            ui.add(egui::Label::new(RichText::new(dir).color(if exists { TEXT } else { FAINT }).size(12.5)).truncate());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(t!("Remover", "Remove")).clicked() {
                                    del = Some(k);
                                }
                                if !exists {
                                    label_muted(ui, t!("não existe", "missing"));
                                }
                            });
                        });
                    }
                    if let Some(k) = del {
                        cfg.app.vst_dirs.remove(k);
                        out.push(SetEdit::VstDirs);
                    }
                    if ui.button(t!("Adicionar pasta", "Add folder")).clicked() {
                        if let Some(p) = rfd::FileDialog::new().set_title(t!("Pasta de plugins VST", "VST plugin folder")).pick_folder() {
                            cfg.app.vst_dirs.push(p.to_string_lossy().into_owned());
                            out.push(SetEdit::VstDirs);
                        }
                    }

                    section(ui, t!("Primeiros passos", "Getting started"));
                    row(ui, t!("Assistente", "Assistant"), |ui| {
                        if ui.button(t!("Abrir", "Open")).on_hover_text(t!("Confere o VB-Cable e o mic, e mede o ruído do ambiente.", "Checks VB-Cable and the mic, and measures room noise.")).clicked() {
                            out.push(SetEdit::Wizard);
                        }
                    });

                    section(ui, t!("Sobre", "About"));
                    label_muted(ui, t!("AMA (Another Mic App). Filtros, GraphicEQ, Loudness correction, convolução e host VST portados do Equalizer APO (GPL-2.0).", "AMA (Another Mic App). Filters, GraphicEQ, Loudness correction, convolution and VST host ported from Equalizer APO (GPL-2.0)."));
                    label_muted(ui, t!("Cancelamento de eco: WebRTC AEC3. Redução de ruído: DeepFilterNet 3.", "Echo cancellation: WebRTC AEC3. Noise reduction: DeepFilterNet 3."));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(t!("Feito por", "Made by")).color(MUTED));
                        ui.label(RichText::new("@ruptvra").font(semibold(14.0)).color(ACCENT));
                        ui.add_space(8.0);
                        ui.hyperlink_to("Buy me a coffee", "https://buymeacoffee.com/ruptvra");
                    });
                    ui.add_space(4.0);
                });
            });
        if !open {
            self.capture = None;
        }
        self.open = open;
        out
    }
}
