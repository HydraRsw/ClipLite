// Registro mínimo a archivo (se recorta solo si crece demasiado).
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

static LOG: Mutex<Option<File>> = Mutex::new(None);

pub fn init(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > 2_000_000 {
            let _ = std::fs::remove_file(path);
        }
    }
    if let Ok(f) = OpenOptions::new().create(true).append(true).open(path) {
        if let Ok(mut g) = LOG.lock() {
            *g = Some(f);
        }
    }
}

pub fn write(msg: &str) {
    let line = format!("[{}] {}\n", crate::util::timestamp(), msg);
    if cfg!(any(not(windows), debug_assertions)) {
        eprint!("{line}");
    }
    if let Ok(mut g) = LOG.lock() {
        if let Some(f) = g.as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

macro_rules! log {
    ($($t:tt)*) => { $crate::log::write(&format!($($t)*)) };
}
