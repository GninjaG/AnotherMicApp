use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub devices: Devices,
    pub aec: Aec,
    pub denoise: Denoise,
    pub output: Output,
    pub buffers: Buffers,
    pub app: App,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Devices {
    /// Parte do nome. Vazio = dispositivo padrão.
    pub mic: String,
    pub speakers: String,
    pub output: String,
    pub mic_raw: bool,
    /// Escuta do mic (vazio = reprodução padrão).
    pub monitor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Aec {
    pub enabled: bool,
    pub tuning: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Denoise {
    pub enabled: bool,
    pub atten_limit_db: f32,
    pub post_filter_beta: f32,
    pub before_aec: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Output {
    pub enabled: bool,
    pub gain_db: f32,
    pub limiter: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Buffers {
    pub out_ms: f64,
    pub ref_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct App {
    pub autostart: bool,
    pub log_file: String,
    pub stats_interval_s: u64,
    /// Preset ativo (presets/<nome>.toml). Vazio = cadeia das seções [aec]/[denoise]/[output].
    pub preset: String,
    /// "perguntar", "bandeja" ou "sair"
    pub on_close: String,
    pub hotkey_mute: String,
    pub hotkey_bypass: String,
    pub vst_dirs: Vec<String>,
    pub show_meters: bool,
    pub show_curve: bool,
    pub show_spectrum: bool,
    pub show_status: bool,
    pub monitor_gain_db: f64,
    pub window: [f32; 2],
    /// "pt" ou "en"
    pub language: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            devices: Devices::default(),
            aec: Aec::default(),
            denoise: Denoise::default(),
            output: Output::default(),
            buffers: Buffers::default(),
            app: App::default(),
        }
    }
}
impl Default for Devices {
    fn default() -> Self {
        Self {
            mic: "Usb Audio Device".into(),
            speakers: "".into(),
            output: "CABLE Input".into(),
            mic_raw: true,
            monitor: "".into(),
        }
    }
}
impl Default for Aec {
    fn default() -> Self {
        Self { enabled: true, tuning: "equilibrado".into() }
    }
}
impl Default for Denoise {
    fn default() -> Self {
        Self { enabled: true, atten_limit_db: 100.0, post_filter_beta: 0.0, before_aec: false }
    }
}
impl Default for Output {
    fn default() -> Self {
        Self { enabled: true, gain_db: 0.0, limiter: true }
    }
}
impl Default for Buffers {
    fn default() -> Self {
        Self { out_ms: 20.0, ref_ms: 25.0 }
    }
}
impl Default for App {
    fn default() -> Self {
        Self {
            autostart: false,
            log_file: "ama.log".into(),
            stats_interval_s: 10,
            preset: "".into(),
            on_close: "perguntar".into(),
            hotkey_mute: "Ctrl+Alt+M".into(),
            hotkey_bypass: "Ctrl+Alt+B".into(),
            vst_dirs: vec![
                r"C:\Program Files\EqualizerAPO\VSTPlugins".into(),
                r"C:\Program Files\VSTPlugins".into(),
                r"C:\Program Files\Steinberg\VstPlugins".into(),
                r"C:\Program Files\Common Files\VST2".into(),
            ],
            show_meters: true,
            show_curve: true,
            show_spectrum: true,
            show_status: true,
            monitor_gain_db: 0.0,
            window: [1280.0, 800.0],
            language: "pt".into(),
        }
    }
}

pub const DEFAULT_TOML: &str = r#"# AMA - ajustes
[devices]
# Parte do nome do dispositivo (sem diferenciar maiúsculas). Vazio = padrão do Windows.
mic = "Usb Audio Device"
# Caixas (referência do AEC, via loopback). Vazio = reprodução padrão.
speakers = ""
output = "CABLE Input"
# Captura o mic sem os efeitos do Windows (modo RAW), se o driver suportar
mic_raw = true

[aec]
enabled = true
# "equilibrado" (preserva mais a sua voz quando os dois falam) ou "webrtc" (padrão do AEC3, corta mais)
tuning = "equilibrado"

[denoise]
enabled = true
# Atenuação máxima do DeepFilterNet em dB (100 = sem limite)
atten_limit_db = 100.0
# Pós-filtro do DeepFilterNet (0 = desligado, típico 0.02)
post_filter_beta = 0.0
# Roda o DeepFilterNet antes do AEC (preserva mais a voz, mas o AEC cancela bem menos eco)
before_aec = false

[output]
enabled = true
gain_db = 0.0
# Limitador em -1 dBFS para nunca clipar
limiter = true

[buffers]
out_ms = 20.0
ref_ms = 25.0

[app]
autostart = false
log_file = "ama.log"
stats_interval_s = 10
"#;

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

impl Config {
    pub fn load_or_create(path: &Path) -> Result<Config, String> {
        if !path.exists() {
            std::fs::write(path, DEFAULT_TOML).map_err(|e| format!("{path:?}: {e}"))?;
        }
        let s = std::fs::read_to_string(path).map_err(|e| format!("{path:?}: {e}"))?;
        toml::from_str(&s).map_err(|e| format!("{path:?}: {e}"))
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let s = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, format!("# AMA - ajustes (editados pela janela)
{s}")).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn resolve(&self, base: &Path, p: &str) -> PathBuf {
        let pb = PathBuf::from(p);
        if pb.is_absolute() { pb } else { base.join(pb) }
    }
}
