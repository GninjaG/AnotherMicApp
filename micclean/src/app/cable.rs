//! Buffer interno do VB-Cable (Max Latency). O driver lê do registro ao iniciar.
use super::theme::*;
use egui::RichText;
use std::sync::mpsc::Receiver;
use windows::Win32::Foundation::*;
use windows::Win32::System::Registry::*;
use windows::core::{HSTRING, PCWSTR, w};

const KEY: PCWSTR = w!(r"SOFTWARE\VB-Audio\Cable");
pub const ARG: &str = "--cable-latency";
const CHOICES: [u32; 4] = [512, 1024, 2048, 7168];
const NO_WINDOW: u32 = 0x0800_0000;

fn read_dword(key: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut v = 0u32;
    let mut n = 4u32;
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key, name, RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut _), Some(&mut n)) };
    (r == ERROR_SUCCESS).then_some(v)
}

fn read_sz(key: PCWSTR, name: PCWSTR) -> Option<String> {
    let mut buf = [0u16; 512];
    let mut n = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key, name, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut n)) };
    (r == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&buf[..(n as usize / 2).saturating_sub(1)]))
}

/// (latência em amostras, taxa interna)
pub fn read() -> Option<(u32, u32)> {
    let lat = read_dword(KEY, w!("VBAudioCableWDM_Latency"))?;
    Some((lat, read_dword(KEY, w!("VBAudioCableWDM_SR")).unwrap_or(48000)))
}

fn run(cmd: &str, args: &[&str]) -> Result<i32, String> {
    use std::os::windows::process::CommandExt;
    let s = std::process::Command::new(cmd).args(args).creation_flags(NO_WINDOW).status().map_err(|e| format!("{cmd}: {e}"))?;
    Ok(s.code().unwrap_or(-1))
}

fn backups() -> std::path::PathBuf {
    let dir = micclean::config::data_dir().join("backups");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn note(s: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(backups().join("vbcable.log")) {
        let _ = writeln!(f, "{} {s}", super::presets::chrono_stamp());
    }
}

fn instance() -> Option<String> {
    let id = read_sz(w!(r"SYSTEM\CurrentControlSet\Services\VBAudioVACMME\Enum"), w!("0"));
    if id.is_none() {
        note("instância do VB-Cable não encontrada");
    }
    id
}

fn pnp(verb: &str, id: &str) -> Result<i32, String> {
    let r = run("pnputil", &[verb, id]);
    note(&format!("pnputil {verb} {id}: {r:?}"));
    r
}

fn restart_device() -> i32 {
    let Some(id) = instance() else { return 2 };
    if pnp("/restart-device", &id) == Ok(0) { 0 } else { 2 }
}

/// O VB-Cable não aceita /restart-device; desativar e reativar faz o driver recarregar.
fn cycle_device() -> i32 {
    let Some(id) = instance() else { return 2 };
    let off = pnp("/disable-device", &id);
    std::thread::sleep(std::time::Duration::from_millis(500));
    // reativa sempre, para nunca deixar o cabo desligado
    let on = pnp("/enable-device", &id);
    if off == Ok(0) && on == Ok(0) { 0 } else { 2 }
}

/// Para o áudio do Windows (que segura o driver), recarrega o VB-Cable e liga de novo.
fn force_restart() -> i32 {
    note(&format!("parar AudioEndpointBuilder: {:?}", run("net", &["stop", "AudioEndpointBuilder", "/y"])));
    let r = cycle_device();
    for svc in ["AudioEndpointBuilder", "Audiosrv"] {
        note(&format!("iniciar {svc}: {:?}", run("net", &["start", svc])));
    }
    r
}

/// Roda no processo elevado. 0 = aplicado, 2 = gravado mas precisa reiniciar o PC, 1 = erro.
pub fn elevated_main(n: &str) -> i32 {
    if n == "force" {
        return force_restart();
    }
    let Ok(n) = n.parse::<u32>() else { return 1 };
    if !CHOICES.contains(&n) {
        return 1;
    }
    let dir = backups();
    let bak = dir.join(format!("vbcable-{}.reg", super::presets::chrono_stamp()));
    if run("reg", &["export", r"HKLM\SOFTWARE\VB-Audio\Cable", &bak.to_string_lossy(), "/y"]) != Ok(0) {
        return 1;
    }
    let r = unsafe { RegSetKeyValueW(HKEY_LOCAL_MACHINE, KEY, w!("VBAudioCableWDM_Latency"), REG_DWORD.0, Some(&n as *const u32 as *const _), 4) };
    if r != ERROR_SUCCESS {
        return 1;
    }
    note(&format!("latência {n} gravada"));
    restart_device()
}

/// Abre o aviso do Windows e espera o processo elevado.
fn apply(arg: String) -> Result<i32, String> {
    use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
    use windows::Win32::UI::Shell::*;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let file = HSTRING::from(exe.as_os_str());
    let params = HSTRING::from(format!("{ARG} {arg}"));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        ..Default::default()
    };
    unsafe {
        if ShellExecuteExW(&mut info).is_err() {
            let e = GetLastError();
            return Err(if e == ERROR_CANCELLED { "cancelado".into() } else { format!("{e:?}") });
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        Ok(code as i32)
    }
}

#[derive(Default)]
pub struct CableUi {
    cur: Option<(u32, u32)>,
    pick: u32,
    loaded: bool,
    busy: Option<Receiver<Result<i32, String>>>,
    msg: Option<(String, bool)>,
    reboot: bool,
}

fn ms(samples: u32, sr: u32) -> f32 {
    samples as f32 * 1000.0 / sr.max(1) as f32
}

impl CableUi {
    /// `false` quando o VB-Cable não está instalado.
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
        if !self.loaded {
            self.cur = read();
            self.pick = self.cur.map(|(l, _)| if l > 1024 { 1024 } else { l }).unwrap_or(1024);
            self.loaded = true;
        }
        let Some((cur, sr)) = self.cur else { return false };
        if let Some(rx) = &self.busy {
            match rx.try_recv() {
                Ok(r) => {
                    self.busy = None;
                    self.cur = read();
                    self.reboot = r == Ok(2);
                    self.msg = Some(match r {
                        Ok(0) => (t!("Pronto. O VB-Cable reiniciou com o buffer novo.", "Done. VB-Cable restarted with the new buffer.").into(), false),
                        Ok(2) => (t!("Gravado. Reinicie o PC para valer.", "Saved. Restart the PC for it to apply.").into(), false),
                        Ok(_) => (t!("Não consegui gravar. Nada mudou.", "Could not save it. Nothing changed.").into(), true),
                        Err(e) if e == "cancelado" => (t!("Cancelado.", "Cancelled.").into(), false),
                        Err(e) => (tr!("Falhou: {e}", "Failed: {e}"), true),
                    });
                }
                Err(_) => {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                }
            }
        }
        ui.horizontal(|ui| {
            let label = |n: u32| {
                let base = format!("{n}  ({:.0} ms)", ms(n, sr));
                match n {
                    1024 => tr!("{base}  recomendado", "{base}  recommended"),
                    7168 => tr!("{base}  padrão", "{base}  default"),
                    _ => base,
                }
            };
            egui::ComboBox::from_id_salt("cable-lat").selected_text(label(self.pick)).width(230.0).show_ui(ui, |ui| {
                for n in CHOICES {
                    ui.selectable_value(&mut self.pick, n, label(n));
                }
            });
            if self.busy.is_some() {
                ui.spinner();
            } else if ui.button(t!("Aplicar", "Apply")).on_hover_text(t!("O Windows vai pedir permissão de administrador. O valor antigo fica salvo em backups.", "Windows will ask for administrator permission. The old value is saved in backups.")).clicked() {
                self.start(ui.ctx(), self.pick.to_string());
            }
        });
        let now = tr!("Agora: {cur} amostras ({:.0} ms)", "Now: {cur} samples ({:.0} ms)", ms(cur, sr));
        match &self.msg {
            Some((m, err)) => {
                ui.label(RichText::new(m).size(12.5).color(if *err { ERR } else { OK }));
            }
            None => label_muted(ui, &now),
        }
        if self.reboot && self.busy.is_none() {
            if ui.button(t!("Forçar agora", "Force it now")).on_hover_text(t!("Para o áudio do Windows por alguns segundos para reiniciar o VB-Cable. Todo o som do PC corta, e alguns jogos ficam sem som até serem reabertos.", "Stops Windows audio for a few seconds to restart VB-Cable. All PC sound cuts out, and some games stay silent until reopened.")).clicked() {
                self.start(ui.ctx(), "force".into());
            }
            label_muted(ui, t!("Corta todo o som do PC por alguns segundos.", "Cuts all PC sound for a few seconds."));
        }
        true
    }

    fn start(&mut self, ctx: &egui::Context, arg: String) {
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(apply(arg));
            c.request_repaint();
        });
        self.busy = Some(rx);
        self.msg = None;
        self.reboot = false;
    }
}
