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
    pub auto_update: bool,
    pub mobile: bool,
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
            auto_update: true,
            mobile: false,
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
                "actualizaciones_automaticas" => self.auto_update = parse_bool(v).unwrap_or(self.auto_update),
                "acceso_movil" | "acceso_móvil" => self.mobile = parse_bool(v).unwrap_or(self.mobile),
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
# Lo más cómodo: clic derecho en el icono de ClipLite > \"Configuración...\".
# Si editas este archivo a mano, después usa \"Recargar configuración\" en ese mismo menú.

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

# ---------------- Actualizaciones y móvil ----------------
# Buscar e instalar actualizaciones de ClipLite automáticamente
actualizaciones_automaticas = {autoup}
# Permitir ver los clips desde el móvil por la Wi-Fi de casa (se empareja con un código QR)
acceso_movil = {mobile}

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
            autoup = b(self.auto_update),
            mobile = b(self.mobile),
            ffmpeg = self.ffmpeg,
        );
        std::fs::write(path, text)
    }
}

impl Config {
    /// Todos los ajustes como JSON, con las mismas claves que el archivo .ini.
    pub fn to_json(&self) -> crate::json::Json {
        use crate::json::Json;
        let st = |s: &str| Json::str(s);
        let list = |v: &[String]| Json::Arr(v.iter().map(|x| Json::str(x)).collect());
        let n = |x: f64| Json::Num(x);
        Json::Obj(vec![
            ("carpeta".into(), st(&self.output_dir.to_string_lossy())),
            ("modo".into(), st(self.mode.as_str())),
            ("monitor".into(), n(self.monitor as f64)),
            ("fps".into(), n(self.fps as f64)),
            ("bitrate_mbps".into(), n(self.bitrate_mbps as f64)),
            ("codec".into(), st(&self.codec)),
            ("encoder".into(), st(&self.encoder)),
            ("cursor".into(), Json::Bool(self.cursor)),
            ("audio".into(), Json::Bool(self.audio)),
            ("desfase_audio_ms".into(), n(self.audio_offset_ms as f64)),
            ("pistas_por_aplicacion".into(), Json::Bool(self.app_tracks)),
            ("apps_con_pista_propia".into(), list(&self.track_apps)),
            ("microfono".into(), Json::Bool(self.mic)),
            ("segundos_antes".into(), n(self.pre as f64)),
            ("segundos_despues".into(), n(self.post as f64)),
            ("segundos_clip_manual".into(), n(self.manual_secs as f64)),
            ("clips_en_modo_completa".into(), Json::Bool(self.clips_in_full)),
            ("atajo_clip".into(), st(&self.hotkey_clip)),
            ("atajo_grabar".into(), st(&self.hotkey_record)),
            ("autodetectar_juegos".into(), Json::Bool(self.autodetect)),
            ("juegos".into(), list(&self.games)),
            ("eventos_lol".into(), list(&self.lol_events)),
            ("eventos_cs2".into(), list(&self.cs2_events)),
            ("abrir_reproductor".into(), Json::Bool(self.open_player)),
            ("borrar_videos_de_mas_de_dias".into(), n(self.clean_days as f64)),
            ("limite_espacio_gb".into(), n(self.limit_gb)),
            ("actualizaciones_automaticas".into(), Json::Bool(self.auto_update)),
            ("acceso_movil".into(), Json::Bool(self.mobile)),
            ("sonido".into(), Json::Bool(self.sound)),
            ("notificaciones".into(), Json::Bool(self.notifications)),
        ])
    }

    /// Aplica ajustes recibidos como JSON (mismas claves que el .ini). Pasa por la misma
    /// validación que el archivo, así que un valor fuera de rango se corrige o se ignora.
    pub fn apply_json(&mut self, j: &crate::json::Json) {
        use crate::json::Json;
        let Json::Obj(fields) = j else { return };
        let clean = |s: &str| s.replace(['\n', '\r'], " ");
        for (k, v) in fields {
            let val = match v {
                Json::Bool(b) => (if *b { "si" } else { "no" }).to_string(),
                Json::Num(x) if x.fract() == 0.0 => format!("{}", *x as i64),
                Json::Num(x) => format!("{x}"),
                Json::Str(s) => clean(s),
                Json::Arr(a) => a.iter().filter_map(|x| x.as_str()).map(|x| clean(x).replace(',', " ")).collect::<Vec<_>>().join(", "),
                _ => continue,
            };
            if k == "carpeta" && val.trim().is_empty() {
                continue;
            }
            self.apply(&format!("{} = {}", clean(k), val));
        }
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
        // JSON: ida y vuelta, y valores fuera de rango corregidos.
        let mut c3 = Config::default();
        c3.apply_json(&c2.to_json());
        assert_eq!(c3.games, c2.games);
        assert_eq!(c3.output_dir, c2.output_dir);
        let j = crate::json::parse(r#"{"fps":999,"modo":"completa","juegos":["x.exe","y z.exe"],"microfono":true,"carpeta":"","atajo_clip":"Alt+F2\nmodo = momentos"}"#).unwrap();
        c3.apply_json(&j);
        assert_eq!(c3.fps, 240);
        assert_eq!(c3.mode, Mode::Completa, "no se pueden colar claves con saltos de línea");
        assert_eq!(c3.games, vec!["x.exe", "y z.exe"]);
        assert!(c3.mic);
        assert_eq!(c3.output_dir, c2.output_dir, "carpeta vacía se ignora");
        let _ = std::fs::remove_dir_all(dir);
    }
}
