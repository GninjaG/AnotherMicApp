use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

static EN: AtomicBool = AtomicBool::new(false);

pub fn set(code: &str) {
    EN.store(code == "en", Relaxed);
}

pub fn en() -> bool {
    EN.load(Relaxed)
}

pub fn pick(pt: &'static str, en: &'static str) -> &'static str {
    if self::en() { en } else { pt }
}

macro_rules! t {
    ($pt:literal, $en:literal) => {
        $crate::app::i18n::pick($pt, $en)
    };
}

macro_rules! tr {
    ($pt:literal, $en:literal $(, $a:expr)* $(,)?) => {
        if $crate::app::i18n::en() { format!($en $(, $a)*) } else { format!($pt $(, $a)*) }
    };
}
