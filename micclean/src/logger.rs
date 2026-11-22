use std::io::Write;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();
static ECHO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn init(path: &std::path::Path, echo: bool) {
    let f = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
    let _ = LOG.set(Mutex::new(f));
    ECHO.store(echo, std::sync::atomic::Ordering::Relaxed);
}

pub fn timestamp() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

/// Não usar na thread de áudio no caminho quente (aloca).
pub fn write(msg: &str) {
    let line = format!("{} {}\n", timestamp(), msg);
    if ECHO.load(std::sync::atomic::Ordering::Relaxed) {
        eprint!("{line}");
    }
    if let Some(m) = LOG.get() {
        if let Ok(mut g) = m.lock() {
            if let Some(f) = g.as_mut() {
                let _ = f.write_all(line.as_bytes());
                let _ = f.flush();
            }
        }
    }
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::logger::write(&format!($($t)*)) };
}
