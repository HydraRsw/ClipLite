// Punto único donde se separa lo específico de Windows de lo genérico.
// En Windows: captura real (Desktop Duplication + WASAPI).
// En otros sistemas: fuentes de prueba para poder probar toda la lógica de grabación y clips.
#![allow(unused_imports)]
use crate::audio::Capture;
use crate::config::Config;
use std::path::{Path, PathBuf};

pub type Fetcher = Box<dyn FnMut(&str) -> Option<String> + Send>;

// ---------------------------------------------------------------- Windows
#[cfg(windows)]
pub const IS_DDA: bool = true;

#[cfg(windows)]
pub fn video_input(cfg: &Config) -> Vec<String> {
    vec![
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        format!("ddagrab=output_idx={}:framerate={}:draw_mouse={}", cfg.monitor, cfg.fps, cfg.cursor as u8),
    ]
}
#[cfg(windows)]
pub fn probe_input(cfg: &Config) -> Vec<String> {
    vec!["-f".into(), "lavfi".into(), "-i".into(), format!("ddagrab=output_idx={}:framerate=10", cfg.monitor)]
}
#[cfg(windows)]
pub use crate::win::{config_dir, default_output_dir, hide_path, install_cs2_cfg, local_time, lol_fetcher, open_audio, open_player};

// ---------------------------------------------------------------- Pruebas (Linux/macOS)
#[cfg(not(windows))]
pub const IS_DDA: bool = false;

#[cfg(not(windows))]
pub fn video_input(cfg: &Config) -> Vec<String> {
    vec!["-f".into(), "lavfi".into(), "-i".into(), format!("testsrc2=size=1280x720:rate={},realtime", cfg.fps)]
}
#[cfg(not(windows))]
pub fn probe_input(_cfg: &Config) -> Vec<String> {
    vec!["-f".into(), "lavfi".into(), "-i".into(), "testsrc2=size=320x240:rate=10".into()]
}
#[cfg(not(windows))]
pub fn local_time() -> (u32, u32, u32, u32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    crate::util::civil_from_unix(secs)
}
#[cfg(not(windows))]
pub fn config_dir() -> PathBuf {
    PathBuf::from("cliplite_data")
}
#[cfg(not(windows))]
pub fn default_output_dir() -> PathBuf {
    PathBuf::from("cliplite_out")
}
#[cfg(not(windows))]
pub fn hide_path(_p: &Path) {}
#[cfg(not(windows))]
pub fn install_cs2_cfg(_port: u16) {}
#[cfg(not(windows))]
pub fn lol_fetcher() -> Option<Fetcher> {
    None
}
#[cfg(not(windows))]
pub fn open_player(html: &Path, session: Option<&str>) {
    log!("Abrir reproductor: {}#s={}", html.display(), session.unwrap_or(""));
}

/// Generador de prueba que imita a WASAPI: entrega paquetes de 10 ms con retraso variable
/// (0-60 ms), su reloj va un poco desviado (CLIPLITE_DRIFT, en tanto por uno) y entre los
/// segundos 3 y 5 no entrega nada (como cuando no suena nada).
#[cfg(not(windows))]
pub fn open_audio() -> Result<Box<dyn Capture>, String> {
    let drift = std::env::var("CLIPLITE_DRIFT").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0005);
    Ok(Box::new(TestTone { start: std::time::Instant::now(), delivered: 0, rng: 0x1234_5678, drift }))
}

#[cfg(not(windows))]
struct TestTone {
    start: std::time::Instant,
    delivered: u64,
    rng: u64,
    drift: f64,
}

#[cfg(not(windows))]
impl Capture for TestTone {
    fn format(&self) -> crate::audio::AudioFormat {
        crate::audio::AudioFormat { rate: 48_000, channels: 2, bits: 32, float: true }
    }
    fn read(&mut self, out: &mut Vec<u8>) -> Result<(), String> {
        self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let latency = (self.rng >> 33) % 60;
        let t = (self.start.elapsed().as_secs_f64() - latency as f64 / 1000.0).max(0.0);
        let produced = (t * 48_000.0 * (1.0 + self.drift)) as u64 / 480 * 480;
        let dev_t = produced as f64 / 48_000.0 / (1.0 + self.drift);
        if (3.0..5.0).contains(&dev_t) {
            self.delivered = self.delivered.max(produced);
            return Ok(());
        }
        while self.delivered < produced {
            let s = ((self.delivered as f64 / 48_000.0) * 440.0 * std::f64::consts::TAU).sin() as f32 * 0.2;
            out.extend_from_slice(&s.to_le_bytes());
            out.extend_from_slice(&s.to_le_bytes());
            self.delivered += 1;
        }
        Ok(())
    }
}
