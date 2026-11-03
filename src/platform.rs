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
pub use crate::win::{autostart_enabled, boost_audio_thread, config_changed, pick_folder_dialog, process_names, set_autostart, trash, config_dir, default_output_dir, hide_path, install_cs2_cfg, local_time, lol_fetcher, open_source, open_url, reveal, track_specs};

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
/// Da prioridad alta (CPU y GPU) al proceso de FFmpeg.
pub fn boost_ffmpeg(child: &std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        crate::win::boost_process(child.as_raw_handle() as isize);
    }
    #[cfg(not(windows))]
    let _ = child;
}
#[cfg(not(windows))]
pub fn boost_audio_thread() {}
#[cfg(not(windows))]
static FAKE_AUTOSTART: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(not(windows))]
pub fn autostart_enabled() -> bool {
    FAKE_AUTOSTART.load(std::sync::atomic::Ordering::SeqCst)
}
#[cfg(not(windows))]
pub fn set_autostart(on: bool) {
    FAKE_AUTOSTART.store(on, std::sync::atomic::Ordering::SeqCst)
}
#[cfg(not(windows))]
pub fn config_changed() {
    log!("Configuración cambiada desde la ventana de ajustes");
}
#[cfg(not(windows))]
pub fn pick_folder_dialog() -> Option<PathBuf> {
    std::env::var_os("CLIPLITE_PICK").map(PathBuf::from)
}
#[cfg(not(windows))]
pub fn process_names() -> Vec<String> {
    vec!["Discord.exe".into(), "League of Legends.exe".into(), "explorer.exe".into()]
}
#[cfg(not(windows))]
pub fn trash(p: &Path) -> Result<(), String> {
    std::fs::remove_file(p).map_err(|e| e.to_string())
}
#[cfg(not(windows))]
pub fn open_url(url: &str) {
    log!("Abrir reproductor: {url}");
}
#[cfg(not(windows))]
pub fn reveal(p: &Path) {
    log!("Mostrar en carpeta: {}", p.display());
}
/// Pistas de prueba: CLIPLITE_TRACKS=1 simula juego, Discord y micrófono.
#[cfg(not(windows))]
pub fn track_specs(cfg: &Config, _game_exe: &str) -> Vec<crate::audio::TrackSpec> {
    use crate::audio::{TrackSpec, TrackSrc};
    let mut v = vec![TrackSpec { name: "Todo".into(), src: TrackSrc::System }];
    if cfg.app_tracks && std::env::var("CLIPLITE_TRACKS").is_ok() {
        v.push(TrackSpec { name: "Juego".into(), src: TrackSrc::Process(1) });
        v.push(TrackSpec { name: "Discord".into(), src: TrackSrc::Process(2) });
        v.push(TrackSpec { name: "Micrófono".into(), src: TrackSrc::Mic });
    }
    v
}

/// Generador de prueba que imita a WASAPI: entrega paquetes de 10 ms con retraso variable
/// (0-60 ms) y su reloj va un poco desviado (CLIPLITE_DRIFT). Cada fuente suena a una frecuencia:
/// sistema 440 Hz (calla entre los segundos 3 y 5), juego 660, Discord 880, micrófono 330.
#[cfg(not(windows))]
pub fn open_source(src: &crate::audio::TrackSrc, _multi: bool) -> Result<Box<dyn Capture>, String> {
    use crate::audio::TrackSrc;
    let drift = std::env::var("CLIPLITE_DRIFT").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0005);
    let (freq, gap) = match src {
        TrackSrc::System => (440.0, true),
        TrackSrc::Process(1) => (660.0, false),
        TrackSrc::Process(_) => (880.0, false),
        TrackSrc::Mic => (330.0, false),
    };
    Ok(Box::new(TestTone { start: std::time::Instant::now(), delivered: 0, rng: 0x1234_5678 ^ freq as u64, drift, freq, gap }))
}

#[cfg(not(windows))]
struct TestTone {
    start: std::time::Instant,
    delivered: u64,
    rng: u64,
    drift: f64,
    freq: f64,
    gap: bool,
}

#[cfg(not(windows))]
impl Capture for TestTone {
    fn format(&self) -> crate::audio::AudioFormat {
        crate::audio::MULTI
    }
    fn read(&mut self, out: &mut Vec<u8>) -> Result<(), String> {
        self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let latency = (self.rng >> 33) % 60;
        let t = (self.start.elapsed().as_secs_f64() - latency as f64 / 1000.0).max(0.0);
        let produced = (t * 48_000.0 * (1.0 + self.drift)) as u64 / 480 * 480;
        let dev_t = produced as f64 / 48_000.0 / (1.0 + self.drift);
        // Como WASAPI: el búfer guarda 1 s; si nadie lo lee a tiempo, lo más antiguo se pierde.
        if produced > self.delivered + 48_000 {
            self.delivered = produced - 48_000;
        }
        if self.gap && (3.0..5.0).contains(&dev_t) {
            self.delivered = self.delivered.max(produced);
            return Ok(());
        }
        while self.delivered < produced {
            let s = ((self.delivered as f64 / 48_000.0) * self.freq * std::f64::consts::TAU).sin() as f32 * 0.2;
            out.extend_from_slice(&s.to_le_bytes());
            out.extend_from_slice(&s.to_le_bytes());
            self.delivered += 1;
        }
        Ok(())
    }
}
