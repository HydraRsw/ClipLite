// Motor de grabación.
//
// FFmpeg graba continuamente en segmentos MPEG-TS de 2 s dentro de una carpeta oculta (.buffer).
//  * Un clip = unir (sin recodificar) los segmentos que cubren [momento - antes, momento + después].
//  * Modo "momentos": los segmentos viejos se borran solos (búfer circular en disco).
//  * Modo "completa": se conservan y al terminar se unen en un único vídeo de la partida.
// Unir segmentos copia bytes: un clip de 20 s tarda milisegundos y no gasta CPU ni GPU.
use crate::audio::{AudioThread, OutInfo, TrackSpec};
use crate::config::{Config, Mode};
use crate::controller::Cmd;
use crate::util::{self, secs_between};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SEG_SECS: u32 = 2;
const MAX_CLIP_SECS: f64 = 120.0;

#[derive(Clone, Debug)]
pub struct Encoder {
    pub label: String,
    pub vargs: Vec<String>,
}

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Cadenas de codificación candidatas, de más eficiente a menos.
/// Las directas por GPU no copian nunca la imagen a la memoria del sistema.
pub fn encoder_variants(cfg: &Config) -> Vec<Encoder> {
    let hevc = cfg.codec == "hevc";
    let b = cfg.bitrate_mbps;
    let (br, mx, bs) = (format!("{b}M"), format!("{}M", b * 3 / 2), format!("{}M", b * 2));
    let rate = |mut v: Vec<String>| {
        v.extend(sv(&["-b:v", &br, "-maxrate", &mx, "-bufsize", &bs]));
        v
    };
    let want = |f: &str| cfg.encoder == "auto" || cfg.encoder == f;
    let mut out = Vec::new();

    if crate::platform::IS_DDA {
        let nv = if hevc { "hevc_nvenc" } else { "h264_nvenc" };
        let amf = if hevc { "hevc_amf" } else { "h264_amf" };
        let qsv = if hevc { "hevc_qsv" } else { "h264_qsv" };
        if want("nvenc") {
            out.push(Encoder {
                label: format!("{nv} (GPU NVIDIA, directo)"),
                vargs: rate(sv(&["-c:v", nv, "-preset", "p4", "-tune", "hq", "-rc", "vbr", "-forced-idr", "1"])),
            });
        }
        if want("amf") {
            out.push(Encoder {
                label: format!("{amf} (GPU AMD, directo)"),
                vargs: rate(sv(&["-c:v", amf, "-quality", "speed", "-rc", "vbr_peak"])),
            });
            out.push(Encoder {
                label: format!("{amf} (GPU AMD)"),
                vargs: rate(sv(&["-vf", "hwdownload,format=bgra", "-c:v", amf, "-quality", "speed", "-rc", "vbr_peak"])),
            });
        }
        if want("qsv") {
            out.push(Encoder {
                label: format!("{qsv} (GPU Intel, directo)"),
                vargs: rate(sv(&["-vf", "hwmap=derive_device=qsv,format=qsv", "-c:v", qsv, "-preset", "veryfast"])),
            });
        }
        if want("nvenc") {
            // Portátiles con dos GPU: la pantalla va por la integrada y NVENC está en la dedicada.
            out.push(Encoder {
                label: format!("{nv} (GPU NVIDIA)"),
                vargs: rate(sv(&["-vf", "hwdownload,format=bgra", "-c:v", nv, "-preset", "p4", "-rc", "vbr", "-forced-idr", "1"])),
            });
        }
        if want("qsv") {
            out.push(Encoder {
                label: format!("{qsv} (GPU Intel)"),
                vargs: rate(sv(&["-vf", "hwdownload,format=bgra,format=nv12", "-c:v", qsv, "-preset", "veryfast"])),
            });
        }
        out.push(Encoder {
            label: "libx264 (CPU — puede bajar FPS)".into(),
            vargs: rate(sv(&[
                "-vf", "hwdownload,format=bgra,format=yuv420p", "-c:v", "libx264", "-preset", "veryfast", "-tune", "zerolatency",
            ])),
        });
    } else {
        out.push(Encoder {
            label: "libx264 (prueba)".into(),
            vargs: rate(sv(&["-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "veryfast", "-tune", "zerolatency"])),
        });
    }
    out
}

/// Prueba cada cadena con una captura real de medio segundo y se queda con la primera que funcione.
pub fn probe_encoder(ffmpeg: &Path, cfg: &Config) -> Option<Encoder> {
    for e in encoder_variants(cfg) {
        let mut args = sv(&["-hide_banner", "-loglevel", "error"]);
        args.extend(crate::platform::probe_input(cfg));
        args.extend(sv(&["-frames:v", "5"]));
        args.extend(e.vargs.clone());
        args.extend(sv(&["-an", "-f", "null", "-"]));
        let child = util::hidden(Command::new(ffmpeg).args(&args))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let ok = match child {
            Ok(mut c) => util::wait_timeout(&mut c, Duration::from_secs(15)).map(|s| s.success()).unwrap_or(false),
            Err(_) => false,
        };
        log!("Prueba de codificador {}: {}", e.label, if ok { "OK" } else { "no disponible" });
        if ok {
            return Some(e);
        }
    }
    None
}

// ------------------------------------------------------------------ Trabajos en segundo plano

pub enum Job {
    Concat {
        dir: PathBuf,
        files: Vec<String>,
        out: PathBuf,
        faststart: bool,
        done: Option<Arc<AtomicBool>>,
        notice: Option<String>,
        /// Nombres de las pistas de audio (el formato intermedio no los guarda).
        titles: Vec<String>,
    },
    RemoveDir(PathBuf),
    /// Añade la sesión a la biblioteca y (opcionalmente) abre el reproductor.
    Library { out_root: PathBuf, entry: LibEntry, open: bool },
    /// Borra los vídeos viejos según la configuración.
    Cleanup { out_root: PathBuf, days: u32, limit_gb: f64 },
}

/// Hilo que une segmentos y limpia carpetas, para no bloquear nunca al controlador.
pub fn spawn_worker(ffmpeg: PathBuf, rx: Receiver<Job>, notify: Sender<Cmd>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("worker".into())
        .spawn(move || {
            let mut n = 0u64;
            while let Ok(job) = rx.recv() {
                match job {
                    Job::Concat { dir, files, out, faststart, done, notice, titles } => {
                        n += 1;
                        let ok = concat(&ffmpeg, &dir, &files, &out, faststart, n, &titles);
                        if let Some(d) = done {
                            d.store(true, Ordering::SeqCst);
                        }
                        if ok {
                            log!("Guardado: {}", out.display());
                            if let Some(msg) = notice {
                                let _ = notify.send(Cmd::Notice(msg));
                            }
                        }
                    }
                    Job::RemoveDir(d) => {
                        for _ in 0..5 {
                            if std::fs::remove_dir_all(&d).is_ok() || !d.exists() {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(500));
                        }
                    }
                    Job::Library { out_root, entry, open } => {
                        let has_video = entry.match_file.as_ref().map_or(false, |m| m.exists())
                            || entry.clips.iter().any(|c| c.file.exists());
                        if !has_video {
                            continue;
                        }
                        match crate::library::add(&out_root, entry) {
                            Ok((_, id)) => {
                                if open {
                                    crate::platform::open_url(&crate::library::player_url(&out_root, Some(&id)));
                                }
                            }
                            Err(e) => log!("No se pudo actualizar la biblioteca: {e}"),
                        }
                    }
                    Job::Cleanup { out_root, days, limit_gb } => {
                        let freed = crate::library::cleanup(&out_root, days, limit_gb, false);
                        if freed.files > 0 {
                            let _ = notify.send(Cmd::Notice(format!("Limpieza: se borraron {}", freed.describe())));
                        }
                    }
                }
            }
        })
        .expect("hilo worker")
}

fn concat(ffmpeg: &Path, dir: &Path, files: &[String], out: &Path, faststart: bool, n: u64, titles: &[String]) -> bool {
    let list = dir.join(format!("concat_{n}.txt"));
    let body: String = files.iter().map(|f| format!("file '{}'\n", f.replace('\'', "'\\''"))).collect();
    if std::fs::write(&list, body).is_err() {
        return false;
    }
    if let Some(p) = out.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let mut args = sv(&["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i"]);
    args.push(list.to_string_lossy().into_owned());
    args.extend(sv(&["-map", "0", "-c", "copy"]));
    if titles.len() > 1 {
        for (k, t) in titles.iter().enumerate() {
            args.extend([format!("-metadata:s:a:{k}"), format!("title={t}")]);
            args.extend([format!("-metadata:s:a:{k}"), format!("handler_name={t}")]);
        }
    }
    if faststart {
        args.extend(sv(&["-movflags", "+faststart"]));
    }
    args.push(out.to_string_lossy().into_owned());
    let ok = match util::hidden(Command::new(ffmpeg).args(&args))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            log!("Error al unir segmentos: {}", String::from_utf8_lossy(&o.stderr).trim());
            false
        }
        Err(e) => {
            log!("No se pudo ejecutar ffmpeg: {e}");
            false
        }
    };
    let _ = std::fs::remove_file(&list);
    ok
}

fn list_ts(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .map(|f| f.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".ts"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Recupera grabaciones que quedaron a medias (corte de luz, cierre forzado…).
pub fn recover_leftovers(buffer_root: &Path, out_root: &Path, jobs: &Sender<Job>) {
    let Ok(rd) = std::fs::read_dir(buffer_root) else { return };
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let info = std::fs::read_to_string(dir.join("session.txt")).unwrap_or_default();
        let mut lines = info.lines();
        let game = lines.next().unwrap_or("Juego").to_string();
        let full = lines.next() == Some("completa");
        let segs = list_ts(&dir);
        if full && !segs.is_empty() {
            let friendly = util::friendly_game(&game);
            let dest = out_root.join(&friendly).join("Partidas");
            let name = format!("{}_{}_recuperada", friendly, e.file_name().to_string_lossy().trim_end_matches(".rec"));
            log!("Recuperando partida interrumpida: {name}");
            let _ = jobs.send(Job::Concat {
                dir: dir.clone(),
                files: segs,
                out: util::unique_path(&dest, &name, "mp4"),
                faststart: false,
                done: None,
                notice: Some("Partida interrumpida recuperada".into()),
                titles: Vec::new(),
            });
        }
        let _ = jobs.send(Job::RemoveDir(dir));
    }
}

// ------------------------------------------------------------------ Datos para el reproductor

pub struct LibClip {
    pub file: PathBuf,
    pub label: String,
    /// Segundo de la partida completa donde empieza el clip (si hay partida completa).
    pub t: f64,
    pub dur: f64,
}

pub struct LibEntry {
    pub id: String,
    pub game: String,
    pub date: String,
    pub duration: f64,
    pub match_file: Option<PathBuf>,
    pub clips: Vec<LibClip>,
    /// (etiqueta, segundo de la sesión)
    pub events: Vec<(String, f64)>,
    pub tracks: Vec<String>,
}

/// Argumentos de FFmpeg para las pistas de audio. Con varias pistas, el flujo de entrada trae
/// 2 canales por pista; se separan con `pan` y cada una se codifica como pista con nombre.
/// La pista 1 es la mezcla completa (más el micrófono, si se graba).
fn audio_track_args(info: &OutInfo) -> Vec<String> {
    let n = info.tracks.len();
    let mut a: Vec<String> = Vec::new();
    if n <= 1 || info.fmt.channels as usize != 2 * n {
        a.extend(sv(&["-map", "1:a", "-c:a", "aac", "-b:a", "160k", "-ac", "2"]));
    } else {
        let mut f = format!("[1:a]asplit={n}");
        for k in 0..n {
            f.push_str(&format!("[x{k}]"));
        }
        for k in 0..n {
            let (l, r) = (2 * k, 2 * k + 1);
            let pan = match (k, info.mic_track()) {
                (0, Some(m)) => format!("c0=c0+c{}|c1=c1+c{}", 2 * m, 2 * m + 1),
                _ => format!("c0=c{l}|c1=c{r}"),
            };
            f.push_str(&format!(";[x{k}]pan=stereo|{pan}[a{k}]"));
        }
        a.extend(["-filter_complex".into(), f]);
        for k in 0..n {
            a.extend(["-map".into(), format!("[a{k}]")]);
        }
        a.extend(sv(&["-c:a", "aac", "-b:a", "160k"]));
        for (k, t) in info.tracks.iter().enumerate() {
            a.extend([format!("-metadata:s:a:{k}"), format!("title={}", t.name)]);
        }
    }
    a.push("-shortest".into());
    a
}

// ------------------------------------------------------------------ Una "parte" = un proceso FFmpeg

/// Si Windows le quita la pantalla a la captura (cambio de pantalla completa, alt-tab, UAC…),
/// FFmpeg termina. Se lanza otra parte y la sesión continúa como si nada.
struct Part {
    idx: u32,
    child: Child,
    audio: Option<AudioThread>,
    origin: Arc<Mutex<Option<Instant>>>,
    spawned: Instant,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    csv_off: u64,
    csv_partial: String,
    logged: bool,
    /// Fotogramas codificados según FFmpeg (para saber si sigue avanzando al cerrar).
    frames: Arc<std::sync::atomic::AtomicU64>,
}

impl Part {
    fn spawn(
        cfg: &Config,
        ffmpeg: &Path,
        enc: &Encoder,
        dir: &Path,
        idx: u32,
        specs: &[TrackSpec],
        strict: bool,
    ) -> Result<(Part, Option<OutInfo>), String> {
        let mut audio = None;
        let mut info: Option<OutInfo> = None;
        if cfg.audio && !specs.is_empty() {
            match AudioThread::start(specs.to_vec(), strict) {
                Ok((a, i)) => {
                    audio = Some(a);
                    info = Some(i);
                }
                Err(e) => log!("Audio no disponible, se graba sin sonido: {e}"),
            }
        }
        let afmt = info.as_ref().map(|i| i.fmt);

        let mut args = sv(&["-hide_banner", "-loglevel", "warning", "-nostats", "-y", "-thread_queue_size", "512"]);
        args.extend(crate::platform::video_input(cfg));
        if let Some(f) = afmt {
            args.extend(sv(&["-thread_queue_size", "1024", "-probesize", "32", "-analyzeduration", "0", "-f", f.ffmpeg_fmt()]));
            args.extend(["-ar".into(), f.rate.to_string(), "-ac".into(), f.channels.to_string()]);
            if cfg.audio_offset_ms != 0 {
                args.extend(["-itsoffset".into(), format!("{:.3}", cfg.audio_offset_ms as f64 / 1000.0)]);
            }
            args.extend(sv(&["-i", "pipe:0"]));
        }
        args.extend(sv(&["-map", "0:v"]));
        args.extend(enc.vargs.iter().cloned());
        args.extend([
            "-g".into(),
            (cfg.fps * SEG_SECS).to_string(),
            "-force_key_frames".into(),
            format!("expr:gte(t,n_forced*{SEG_SECS})"),
        ]);
        if let Some(i) = &info {
            args.extend(audio_track_args(i));
        }
        args.extend(sv(&["-progress", "pipe:1", "-stats_period", "0.5"]));
        args.extend(sv(&["-f", "segment", "-segment_time", "2", "-segment_format", "mpegts", "-segment_list"]));
        args.push(format!("p{idx:03}.csv"));
        args.extend(sv(&["-segment_list_type", "csv", "-reset_timestamps", "0"]));
        args.push(format!("p{idx:03}_%06d.ts"));

        let mut child = util::hidden(Command::new(ffmpeg).args(&args))
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("No se pudo iniciar ffmpeg: {e}"))?;
        let spawned = Instant::now();
        crate::platform::boost_ffmpeg(&child);

        // Reloj del vídeo: FFmpeg informa cuántos fotogramas ha codificado (a ritmo constante),
        // así que fotograma 0 = ahora - fotogramas/fps. Se queda el valor más temprano observado.
        let origin = Arc::new(Mutex::new(None::<Instant>));
        let frames = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let fps = cfg.fps as f64;
        if let Some(out) = child.stdout.take() {
            let origin = origin.clone();
            let frames = frames.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let Some(v) = line.strip_prefix("frame=") else { continue };
                    let Ok(n) = v.trim().parse::<u64>() else { continue };
                    frames.store(n, Ordering::SeqCst);
                    let now = Instant::now();
                    let secs = n as f64 / fps;
                    if n == 0 || secs > now.duration_since(spawned).as_secs_f64() + 1.0 {
                        continue;
                    }
                    if let Some(c) = now.checked_sub(Duration::from_secs_f64(secs)) {
                        let c = c.max(spawned);
                        let mut o = origin.lock().unwrap();
                        if o.map_or(true, |x| c < x) {
                            *o = Some(c);
                        }
                    }
                }
            });
        }
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(err) = child.stderr.take() {
            let tail = stderr_tail.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let mut t = tail.lock().unwrap();
                    if t.len() >= 30 {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            });
        }
        if let Some(a) = audio.as_mut() {
            if let Some(stdin) = child.stdin.take() {
                a.attach(stdin, origin.clone());
            }
        }
        let part = Part {
            idx,
            child,
            audio,
            origin,
            spawned,
            stderr_tail,
            csv_off: 0,
            csv_partial: String::new(),
            logged: false,
            frames,
        };
        Ok((part, info))
    }

    fn origin(&mut self, fallback: bool) -> Option<Instant> {
        let o = *self.origin.lock().unwrap();
        if let (Some(o), false) = (o, self.logged) {
            self.logged = true;
            log!("Parte {}: el vídeo empezó {:.0} ms tras lanzar ffmpeg", self.idx, secs_between(self.spawned, o) * 1000.0);
        }
        o.or(if fallback { Some(self.spawned + Duration::from_millis(800)) } else { None })
    }

    /// Lee los segmentos terminados y los añade con su hora real.
    fn read_csv(&mut self, dir: &Path, out: &mut Vec<Seg>, fallback: bool) {
        let Some(origin) = self.origin(fallback) else { return };
        let Ok(mut f) = std::fs::File::open(dir.join(format!("p{:03}.csv", self.idx))) else { return };
        if f.seek(SeekFrom::Start(self.csv_off)).is_err() {
            return;
        }
        let mut s = String::new();
        if f.read_to_string(&mut s).is_err() {
            return;
        }
        self.csv_off += s.len() as u64;
        self.csv_partial.push_str(&s);
        while let Some(i) = self.csv_partial.find('\n') {
            let line: String = self.csv_partial.drain(..=i).collect();
            let mut it = line.trim().split(',');
            if let (Some(name), Some(a), Some(b)) = (it.next(), it.next(), it.next()) {
                if let (Ok(a), Ok(b)) = (a.parse::<f64>(), b.parse::<f64>()) {
                    if b > a && a >= 0.0 {
                        out.push(Seg {
                            file: name.trim_matches('"').to_string(),
                            start: origin + Duration::from_secs_f64(a),
                            end: origin + Duration::from_secs_f64(b),
                            dur: b - a,
                        });
                    }
                }
            }
        }
    }

    fn tail(&self) -> String {
        self.stderr_tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n")
    }

    fn stop(&mut self) {
        if let Some(a) = self.audio.take() {
            a.stop();
        } else if let Some(mut stdin) = self.child.stdin.take() {
            let _ = stdin.write_all(b"q");
        }
        // FFmpeg puede tener audio pendiente de procesar (si se retrasó durante la partida):
        // se le espera mientras siga avanzando, hasta 90 s; si se queda parado 4 s, se cierra.
        let start = Instant::now();
        let mut last = self.frames.load(Ordering::SeqCst);
        let mut last_change = Instant::now();
        loop {
            if matches!(self.child.try_wait(), Ok(Some(_)) | Err(_)) {
                break;
            }
            let f = self.frames.load(Ordering::SeqCst);
            if f != last {
                last = f;
                last_change = Instant::now();
            }
            let waited = start.elapsed();
            if waited > Duration::from_secs(90) || (waited > Duration::from_secs(8) && last_change.elapsed() > Duration::from_secs(4)) {
                let _ = self.child.kill();
                let _ = self.child.wait();
                log!("FFmpeg no respondió; se forzó el cierre");
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let waited = start.elapsed().as_secs_f64();
        if waited > 3.0 {
            log!("FFmpeg tardó {waited:.1} s en terminar de guardar");
        }
    }
}

// ------------------------------------------------------------------ Sesión de grabación

struct Seg {
    file: String,
    start: Instant,
    end: Instant,
    dur: f64,
}

struct ClipReq {
    start: Instant,
    end: Instant,
    label: String,
    prio: u8,
}

pub struct Session {
    part: Option<Part>,
    next_idx: u32,
    fast_fails: u32,
    cfg: Config,
    ffmpeg: PathBuf,
    enc: Encoder,
    dir: PathBuf,
    out_root: PathBuf,
    game: String,
    started_ts: String,
    started_at: Instant,
    pub mode: Mode,
    segs: Vec<Seg>,
    pruned: usize,
    pending: Vec<ClipReq>,
    inflight: Vec<(Arc<AtomicBool>, Instant)>,
    keep: Duration,
    events: Vec<(String, Instant)>,
    clips: Vec<(PathBuf, String, Instant, Instant)>,
    /// Pistas fijadas por la primera parte (todas las partes deben tener las mismas).
    specs: Vec<TrackSpec>,
    track_names: Vec<String>,
}

impl Session {
    pub fn start(cfg: &Config, ffmpeg: &Path, enc: &Encoder, game_exe: &str) -> Result<Session, String> {
        let buffer_root = cfg.output_dir.join(".buffer");
        let started_ts = util::timestamp();
        let dir = util::unique_path(&buffer_root, &started_ts, "rec");
        std::fs::create_dir_all(&dir).map_err(|e| format!("No se pudo crear {}: {e}", dir.display()))?;
        crate::platform::hide_path(&buffer_root);
        let _ = std::fs::write(dir.join("session.txt"), format!("{game_exe}\n{}\n", cfg.mode.as_str()));
        let wanted = crate::platform::track_specs(cfg, game_exe);
        let (part, info) = Part::spawn(cfg, ffmpeg, enc, &dir, 0, &wanted, false)?;
        let specs = info.as_ref().map(|i| i.tracks.clone()).unwrap_or_default();
        let track_names: Vec<String> = specs.iter().map(|t| t.name.clone()).collect();
        if track_names.len() > 1 {
            log!("Pistas de audio: {}", track_names.join(", "));
        }
        log!("Grabando {game_exe} con {} en {}", enc.label, dir.display());
        Ok(Session {
            part: Some(part),
            next_idx: 1,
            fast_fails: 0,
            cfg: cfg.clone(),
            ffmpeg: ffmpeg.to_path_buf(),
            enc: enc.clone(),
            dir,
            out_root: cfg.output_dir.clone(),
            game: util::friendly_game(game_exe),
            started_ts,
            started_at: Instant::now(),
            mode: cfg.mode,
            segs: Vec::new(),
            pruned: 0,
            pending: Vec::new(),
            inflight: Vec::new(),
            keep: Duration::from_secs(cfg.pre.max(cfg.manual_secs) as u64 + 10),
            events: Vec::new(),
            clips: Vec::new(),
            specs,
            track_names,
        })
    }

    pub fn game(&self) -> &str {
        &self.game
    }

    /// Pide un clip alrededor de `at`. Momentos cercanos se fusionan en un único clip.
    pub fn request_clip(&mut self, at: Instant, pre: f64, post: f64, label: &str, prio: u8, manual: bool) {
        self.events.push((label.to_string(), at));
        if self.mode == Mode::Completa && !self.cfg.clips_in_full && !manual {
            return;
        }
        let start = at.checked_sub(Duration::from_secs_f64(pre)).unwrap_or(at);
        let end = at + Duration::from_secs_f64(post);
        for p in self.pending.iter_mut() {
            let overlaps = secs_between(p.end, start) <= 1.0 && secs_between(end, p.start) <= 1.0;
            let new_len = secs_between(p.start.min(start), p.end.max(end));
            if overlaps && new_len <= MAX_CLIP_SECS {
                p.start = p.start.min(start);
                p.end = p.end.max(end);
                if prio > p.prio {
                    p.prio = prio;
                    p.label = label.to_string();
                }
                return;
            }
        }
        self.pending.push(ClipReq { start, end, label: label.to_string(), prio });
    }

    fn flush(&mut self, jobs: &Sender<Job>, force: bool) {
        let last_end = self.segs.last().map(|s| s.end);
        let mut i = 0;
        while i < self.pending.len() {
            if force || last_end.map_or(false, |e| e >= self.pending[i].end) {
                let p = self.pending.remove(i);
                self.make_clip(jobs, p.start, p.end, &p.label);
            } else {
                i += 1;
            }
        }
    }

    fn make_clip(&mut self, jobs: &Sender<Job>, s: Instant, e: Instant, label: &str) {
        let files: Vec<String> = self.segs[self.pruned..]
            .iter()
            .filter(|g| g.end > s && g.start < e)
            .map(|g| g.file.clone())
            .collect();
        if files.is_empty() {
            log!("Clip '{label}' descartado: no hay vídeo grabado para ese momento");
            return;
        }
        let first = self.segs[self.pruned..].iter().find(|g| g.end > s).map(|g| g.start).unwrap_or(s);
        let last = self.segs[self.pruned..].iter().filter(|g| g.start < e).map(|g| g.end).max().unwrap_or(e);
        let dest = self.out_root.join(&self.game).join("Clips");
        let stem = format!("{}_{}_{}", self.game, util::timestamp(), label);
        let out = util::unique_path(&dest, &stem, "mp4");
        let done = Arc::new(AtomicBool::new(false));
        self.inflight.push((done.clone(), first));
        self.clips.push((out.clone(), label.to_string(), first, last));
        let _ = jobs.send(Job::Concat {
            dir: self.dir.clone(),
            files,
            out,
            faststart: true,
            done: Some(done),
            notice: Some(format!("Clip guardado: {label}")),
            titles: self.track_names.clone(),
        });
    }

    fn prune(&mut self) {
        if self.mode != Mode::Momentos {
            return;
        }
        self.inflight.retain(|(d, _)| !d.load(Ordering::SeqCst));
        let mut cutoff = Instant::now().checked_sub(self.keep).unwrap_or(self.started_at);
        for p in &self.pending {
            cutoff = cutoff.min(p.start);
        }
        for (_, s) in &self.inflight {
            cutoff = cutoff.min(*s);
        }
        while self.pruned < self.segs.len() && self.segs[self.pruned].end < cutoff {
            let _ = std::fs::remove_file(self.dir.join(&self.segs[self.pruned].file));
            self.pruned += 1;
        }
    }

    /// Se llama periódicamente. Devuelve false si la captura falla una y otra vez.
    pub fn tick(&mut self, jobs: &Sender<Job>) -> bool {
        if let Some(p) = self.part.as_mut() {
            p.read_csv(&self.dir, &mut self.segs, false);
        }
        self.flush(jobs, false);
        self.prune();

        let exited = match self.part.as_mut() {
            Some(p) => matches!(p.child.try_wait(), Ok(Some(_))),
            None => true,
        };
        if !exited {
            return true;
        }
        if let Some(mut p) = self.part.take() {
            p.read_csv(&self.dir, &mut self.segs, true);
            let lived = p.spawned.elapsed();
            log!("La captura se interrumpió tras {:.0} s; se reanuda en la misma partida. FFmpeg dijo:\n{}", lived.as_secs_f64(), p.tail());
            p.stop();
            if lived < Duration::from_secs(10) {
                self.fast_fails += 1;
            } else {
                self.fast_fails = 0;
            }
        }
        if self.fast_fails >= 6 {
            log!("La captura falla una y otra vez; se detiene la grabación");
            return false;
        }
        if self.fast_fails > 0 {
            std::thread::sleep(Duration::from_millis(300 * self.fast_fails as u64));
        }
        match Part::spawn(&self.cfg, &self.ffmpeg, &self.enc, &self.dir, self.next_idx, &self.specs, true) {
            Ok((p, _)) => {
                self.next_idx += 1;
                self.part = Some(p);
                true
            }
            Err(e) => {
                log!("{e}");
                false
            }
        }
    }

    pub fn set_mode(&mut self, m: Mode) {
        self.mode = m;
        let _ = std::fs::write(self.dir.join("session.txt"), format!("{}\n{}\n", self.game, m.as_str()));
    }

    /// Segundo dentro del vídeo de la partida (segmentos unidos) que corresponde a `at`.
    fn file_time(&self, at: Instant) -> f64 {
        let mut acc = 0.0;
        for g in &self.segs[self.pruned..] {
            if at < g.start {
                return acc;
            }
            if at < g.end {
                return acc + secs_between(g.start, at);
            }
            acc += g.dur;
        }
        acc
    }

    /// Detiene la grabación, guarda los clips pendientes, la partida (modo completa)
    /// y registra la sesión en la biblioteca del reproductor.
    pub fn stop(mut self, jobs: &Sender<Job>, open_player: bool) {
        if let Some(mut p) = self.part.take() {
            let dir = self.dir.clone();
            p.read_csv(&dir, &mut self.segs, true);
            p.stop();
            p.read_csv(&dir, &mut self.segs, true);
        }
        self.flush(jobs, true);

        let full = self.mode == Mode::Completa && self.pruned < self.segs.len();
        let mut match_file = None;
        if full {
            let files: Vec<String> = self.segs[self.pruned..].iter().map(|g| g.file.clone()).collect();
            let dest = self.out_root.join(&self.game).join("Partidas");
            let out = util::unique_path(&dest, &format!("{}_{}", self.game, self.started_ts), "mp4");
            match_file = Some(out.clone());
            let _ = jobs.send(Job::Concat {
                dir: self.dir.clone(),
                files,
                out,
                faststart: false,
                done: None,
                notice: Some(format!("Partida guardada ({})", self.game)),
                titles: self.track_names.clone(),
            });
        }
        let _ = jobs.send(Job::RemoveDir(self.dir.clone()));

        // Tiempos para la línea del reproductor: posición dentro del vídeo de la partida si existe;
        // si no, segundos desde que empezó la sesión.
        let base = self.segs.first().map(|g| g.start).unwrap_or(self.started_at);
        let t_of = |at: Instant| if full { self.file_time(at) } else { secs_between(base, at).max(0.0) };
        let duration = if full {
            self.segs[self.pruned..].iter().map(|g| g.dur).sum()
        } else {
            self.segs.last().map(|g| secs_between(base, g.end)).unwrap_or(0.0).max(0.0)
        };
        let entry = LibEntry {
            id: util::sanitize(&format!("{}_{}", self.game, self.started_ts)).replace(' ', "_"),
            game: self.game.clone(),
            date: self.started_ts.clone(),
            duration,
            match_file,
            clips: self
                .clips
                .iter()
                .map(|(f, l, s, e)| LibClip { file: f.clone(), label: l.clone(), t: t_of(*s), dur: secs_between(*s, *e) })
                .collect(),
            events: self.events.iter().map(|(l, at)| (l.clone(), t_of(*at))).collect(),
            tracks: self.track_names.clone(),
        };
        let _ = jobs.send(Job::Library { out_root: self.out_root.clone(), entry, open: open_player });
        log!("Grabación detenida ({})", self.game);
    }
}
