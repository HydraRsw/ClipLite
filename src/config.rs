// Configuración en un archivo .ini sencillo y comentado (editable a mano).
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Graba la partida entera (y además guarda clips de los momentos).
    Completa,
    /// Mantiene solo un búfer corto y guarda únicamente los momentos importantes.
    Momentos,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Completa => "completa",
            Mode::Momentos => "momentos",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub output_dir: PathBuf,
    pub mode: Mode,
    pub monitor: u32,
    pub fps: u32,
    pub bitrate_mbps: u32,
    pub codec: String,
    pub encoder: String,
    pub audio: bool,
    pub audio_offset_ms: i32,
    pub cursor: bool,
    pub pre: u32,
    pub post: u32,
    pub manual_secs: u32,
    pub hotkey_clip: String,
    pub hotkey_record: String,
    pub autodetect: bool,
    pub games: Vec<String>,
    pub lol_events: Vec<String>,
    pub cs2_events: Vec<String>,
    pub cs2_port: u16,
    pub clips_in_full: bool,
    pub sound: bool,
    pub notifications: bool,
    pub open_player: bool,
    pub app_tracks: bool,
    pub track_apps: Vec<String>,
    pub mic: bool,
    pub clean_days: u32,
    pub limit_gb: f64,
    pub ffmpeg: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            output_dir: crate::platform::default_output_dir(),
            mode: Mode::Momentos,
            monitor: 0,
            fps: 60,
            bitrate_mbps: 20,
            codec: "h264".into(),
            encoder: "auto".into(),
            audio: true,
            audio_offset_ms: 0,
            cursor: true,
            pre: 15,
            post: 5,
            manual_secs: 30,
            hotkey_clip: "Ctrl+F10".into(),
            hotkey_record: "Ctrl+F9".into(),
            autodetect: true,
            games: [
                "League of Legends.exe",
                "VALORANT-Win64-Shipping.exe",
                "cs2.exe",
                "r5apex.exe",
                "r5apex_dx12.exe",
                "FortniteClient-Win64-Shipping.exe",
                "Overwatch.exe",
                "RocketLeague.exe",
                "dota2.exe",
                "RainbowSix.exe",
                "Marvel-Win64-Shipping.exe",
                "TslGame.exe",
                "cod.exe",
                "GTA5.exe",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            lol_events: ["kills", "multikills", "objetivos"].iter().map(|s| s.to_string()).collect(),
            cs2_events: ["kills"].iter().map(|s| s.to_string()).collect(),
            cs2_port: 3000,
            clips_in_full: true,
            sound: true,
            notifications: true,
            open_player: true,
            app_tracks: true,
            track_apps: ["Discord.exe", "Spotify.exe"].iter().map(|s| s.to_string()).collect(),
            mic: false,
            clean_days: 0,
            limit_gb: 0.0,
            ffmpeg: String::new(),
        }
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_lowercase().as_str() {
        "true" | "si" | "sí" | "1" | "yes" | "on" => Some(true),
        "false" | "no" | "0" | "off" => Some(false),
        _ => None,
    }
}

fn parse_list(v: &str) -> Vec<String> {
    v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

impl Config {
    pub fn load_or_create(path: &Path) -> Config {
        let mut c = Config::default();
        match std::fs::read_to_string(path) {
            Ok(text) => {
                c.apply(&text);
                // Reescribe para añadir claves nuevas que falten, manteniendo los valores.
                let _ = c.save(path);
            }
            Err(_) => {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = c.save(path);
            }
        }
        c
    }

    fn apply(&mut self, text: &str) {
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim().to_lowercase(), v.trim());
            let num = |d: u32| v.parse::<u32>().unwrap_or(d);
            match k.as_str() {
                "carpeta" if !v.is_empty() => self.output_dir = PathBuf::from(v),
                "modo" => {
                    self.mode = if v.to_lowercase().starts_with('c') { Mode::Completa } else { Mode::Momentos }
                }
                "monitor" => self.monitor = num(self.monitor),
                "fps" => self.fps = num(self.fps).clamp(10, 240),
                "bitrate_mbps" => self.bitrate_mbps = num(self.bitrate_mbps).clamp(2, 150),
                "codec" => self.codec = if v.eq_ignore_ascii_case("hevc") { "hevc".into() } else { "h264".into() },
                "encoder" => self.encoder = v.to_lowercase(),
                "audio" => self.audio = parse_bool(v).unwrap_or(self.audio),
                "desfase_audio_ms" => self.audio_offset_ms = v.parse().unwrap_or(0),
                "cursor" => self.cursor = parse_bool(v).unwrap_or(self.cursor),
                "segundos_antes" => self.pre = num(self.pre).clamp(1, 120),
                "segundos_despues" => self.post = num(self.post).clamp(0, 60),
                "segundos_clip_manual" => self.manual_secs = num(self.manual_secs).clamp(5, 300),
                "atajo_clip" => self.hotkey_clip = v.to_string(),
                "atajo_grabar" => self.hotkey_record = v.to_string(),
                "autodetectar_juegos" => self.autodetect = parse_bool(v).unwrap_or(self.autodetect),
                "juegos" => self.games = parse_list(v),
                "eventos_lol" => self.lol_events = parse_list(&v.to_lowercase()),
                "eventos_cs2" => self.cs2_events = parse_list(&v.to_lowercase()),
                "puerto_cs2" => self.cs2_port = v.parse().unwrap_or(self.cs2_port),
                "clips_en_modo_completa" => self.clips_in_full = parse_bool(v).unwrap_or(self.clips_in_full),
                "sonido" => self.sound = parse_bool(v).unwrap_or(self.sound),
                "notificaciones" => self.notifications = parse_bool(v).unwrap_or(self.notifications),
                "pistas_por_aplicacion" => self.app_tracks = parse_bool(v).unwrap_or(self.app_tracks),
                "apps_con_pista_propia" => self.track_apps = parse_list(v),
                "microfono" | "micrófono" => self.mic = parse_bool(v).unwrap_or(self.mic),
                "borrar_videos_de_mas_de_dias" => self.clean_days = v.parse().unwrap_or(self.clean_days),
                "limite_espacio_gb" => self.limit_gb = v.replace(',', ".").parse().unwrap_or(self.limit_gb),
                "abrir_reproductor" => self.open_player = parse_bool(v).unwrap_or(self.open_player),
                "ffmpeg" => self.ffmpeg = v.to_string(),
                _ => {}
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let b = |x: bool| if x { "si" } else { "no" };
        let text = format!(
            "# ClipLite - configuración
# Tras editar, haz clic derecho en el icono de la bandeja > \"Recargar configuración\".

# Carpeta donde se guardan los clips y las partidas
carpeta = {carpeta}

# completa = graba toda la partida (y también guarda clips de los momentos)
# momentos = solo guarda los momentos importantes (usa muy poco disco)
modo = {modo}

# ---------------- Vídeo ----------------
# Monitor a capturar (0 = principal)
monitor = {monitor}
fps = {fps}
# Calidad: 1080p60 -> 15-25 | 1440p60 -> 25-35 | 4K -> 40-60
bitrate_mbps = {bitrate}
# h264 (máxima compatibilidad: Discord, WhatsApp...) o hevc (mitad de tamaño, misma calidad)
codec = {codec}
# auto | nvenc (NVIDIA) | amf (AMD) | qsv (Intel) | cpu (no recomendado: afecta a los FPS)
encoder = {encoder}
# Dibujar el puntero del ratón
cursor = {cursor}

# ---------------- Audio ----------------
# Graba el sonido del PC (lo mismo que oyes)
audio = {audio}
# Si notas el audio desincronizado, ajústalo aquí (milisegundos, puede ser negativo)
desfase_audio_ms = {offset}
# Guardar el juego y cada aplicación en pistas de audio separadas (para silenciarlas luego en el
# reproductor). La pista 1 siempre es la mezcla completa. Requiere Windows 11 o Windows 10 reciente.
pistas_por_aplicacion = {apptracks}
# Aplicaciones (además del juego) con pista propia, si están abiertas al empezar la partida
apps_con_pista_propia = {trackapps}
# Grabar también tu micrófono (en su propia pista y en la mezcla)
microfono = {mic}

# ---------------- Clips ----------------
# Segundos antes y después de cada momento importante
segundos_antes = {pre}
segundos_despues = {post}
# Segundos que guarda el atajo de clip manual
segundos_clip_manual = {manual}
# En modo completa, guardar también los clips de momentos por separado
clips_en_modo_completa = {cif}

# ---------------- Atajos ----------------
# Formato: Ctrl/Alt/Shift/Win + tecla (F1-F24, A-Z, 0-9, Insert, Home, PageUp...)
atajo_clip = {hk_clip}
atajo_grabar = {hk_rec}

# ---------------- Detección automática ----------------
# Empieza a grabar sola cuando se abre uno de estos juegos y para al cerrarlo
autodetectar_juegos = {auto}
juegos = {juegos}

# Momentos automáticos de League of Legends: kills, multikills, muertes, asistencias, objetivos
eventos_lol = {lol}
# Momentos automáticos de Counter-Strike 2: kills, muertes  (vacío = desactivado)
eventos_cs2 = {cs2}
puerto_cs2 = {port}

# ---------------- Reproductor ----------------
# Abrir el reproductor con la partida y sus clips al cerrar el juego
abrir_reproductor = {player}

# ---------------- Limpieza ----------------
# Borrar automáticamente los vídeos de partidas con más de estos días (0 = nunca).
# Los clips marcados como favoritos (☆ en el reproductor) nunca se borran.
borrar_videos_de_mas_de_dias = {clean}
# Si los vídeos ocupan más de estos GB, borrar empezando por los más antiguos (0 = sin límite)
limite_espacio_gb = {limit}

# ---------------- Otros ----------------
sonido = {sonido}
notificaciones = {notif}
# Ruta a ffmpeg.exe (vacío = buscar junto al programa o en el PATH)
ffmpeg = {ffmpeg}
",
            carpeta = self.output_dir.display(),
            modo = self.mode.as_str(),
            monitor = self.monitor,
            fps = self.fps,
            bitrate = self.bitrate_mbps,
            codec = self.codec,
            encoder = self.encoder,
            cursor = b(self.cursor),
            audio = b(self.audio),
            offset = self.audio_offset_ms,
            pre = self.pre,
            post = self.post,
            manual = self.manual_secs,
            cif = b(self.clips_in_full),
            hk_clip = self.hotkey_clip,
            hk_rec = self.hotkey_record,
            auto = b(self.autodetect),
            juegos = self.games.join(", "),
            lol = self.lol_events.join(", "),
            cs2 = self.cs2_events.join(", "),
            port = self.cs2_port,
            sonido = b(self.sound),
            notif = b(self.notifications),
            player = b(self.open_player),
            apptracks = b(self.app_tracks),
            trackapps = self.track_apps.join(", "),
            mic = b(self.mic),
            clean = self.clean_days,
            limit = self.limit_gb,
            ffmpeg = self.ffmpeg,
        );
        std::fs::write(path, text)
    }
}

pub fn config_path() -> PathBuf {
    crate::platform::config_dir().join("config.ini")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("cliplite_cfg_{}", std::process::id()));
        let p = dir.join("config.ini");
        let mut c = Config::load_or_create(&p);
        c.mode = Mode::Completa;
        c.output_dir = PathBuf::from(r"D:\Mis Clips");
        c.games = vec!["a.exe".into(), "b c.exe".into()];
        c.save(&p).unwrap();
        let c2 = Config::load_or_create(&p);
        assert_eq!(c2.mode, Mode::Completa);
        assert_eq!(c2.output_dir, PathBuf::from(r"D:\Mis Clips"));
        assert_eq!(c2.games, vec!["a.exe", "b c.exe"]);
        assert_eq!(c2.hotkey_clip, "Ctrl+F10");
        let _ = std::fs::remove_dir_all(dir);
    }
}
