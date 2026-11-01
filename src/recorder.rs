// Motor de grabación.
//
// FFmpeg graba continuamente en segmentos MPEG-TS de 2 s dentro de una carpeta oculta (.buffer).
//  * Un clip = unir (sin recodificar) los segmentos que cubren [momento - antes, momento + después].
//  * Modo "momentos": los segmentos viejos se borran solos (búfer circular en disco).
//  * Modo "completa": se conservan y al terminar se unen en un único vídeo de la partida.
// Unir segmentos copia bytes: un clip de 20 s tarda milisegundos y no gasta CPU ni GPU.
use crate::audio::AudioThread;
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
                "-vf", "hwdownload,format=bgra,format=yuv420p", "-c:v", "libx264", "-preset", "veryfast",
            ])),
        });
    } else {
        out.push(Encoder {
            label: "libx264 (prueba)".into(),
            vargs: rate(sv(&["-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "veryfast"])),
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
    },
    RemoveDir(PathBuf),
}

/// Hilo que une segmentos y limpia carpetas, para no bloquear nunca al controlador.
pub fn spawn_worker(ffmpeg: PathBuf, rx: Receiver<Job>, notify: Sender<Cmd>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("worker".into())
        .spawn(move || {
            let mut n = 0u64;
            while let Ok(job) = rx.recv() {
                match job {
                    Job::Concat { dir, files, out, faststart, done, notice } => {
                        n += 1;
                        let list = dir.join(format!("concat_{n}.txt"));
                        let body: String = files.iter().map(|f| format!("file '{}'\n", f.replace('\'', "'\\''"))).collect();
                        let ok = std::fs::write(&list, body).is_ok() && {
                            if let Some(p) = out.parent() {
                                let _ = std::fs::create_dir_all(p);
                            }
                            let mut args = sv(&["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i"]);
                            args.push(list.to_string_lossy().into_owned());
                            args.extend(sv(&["-map", "0", "-c", "copy"]));
                            if faststart {
                                args.extend(sv(&["-movflags", "+faststart"]));
                            }
                            args.push(out.to_string_lossy().into_owned());
                            match util::hidden(Command::new(&ffmpeg).args(&args))
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
                            }
                        };
                        let _ = std::fs::remove_file(&list);
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
                }
            }
        })
        .expect("hilo worker")
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
        let mut segs: Vec<String> = std::fs::read_dir(&dir)
            .map(|r| {
                r.flatten()
                    .map(|f| f.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with("seg_") && n.ends_with(".ts"))
                    .collect()
            })
            .unwrap_or_default();
        segs.sort();
        if full && !segs.is_empty() {
            let friendly = util::friendly_game(&game);
            let dest = out_root.join(&friendly).join("Partidas");
            let name = format!("{}_{}_recuperada", friendly, e.file_name().to_string_lossy());
            log!("Recuperando partida interrumpida: {name}");
            let _ = jobs.send(Job::Concat {
                dir: dir.clone(),
                files: segs,
                out: util::unique_path(&dest, &name, "mp4"),
                faststart: false,
                done: None,
                notice: Some("Partida interrumpida recuperada".into()),
            });
        }
        let _ = jobs.send(Job::RemoveDir(dir));
    }
}

// ------------------------------------------------------------------ Sesión de grabación

struct Seg {
    file: String,
    start: f64,
    end: f64,
}

struct ClipReq {
    start: Instant,
    end: Instant,
    label: String,
    prio: u8,
}

pub struct Session {
    child: Child,
    audio: Option<AudioThread>,
    origin: Arc<Mutex<Option<Instant>>>,
    spawned: Instant,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    dir: PathBuf,
    out_root: PathBuf,
    game: String,
    started_ts: String,
    pub mode: Mode,
    clips_in_full: bool,
    segs: Vec<Seg>,
    csv_off: u64,
    csv_partial: String,
    pruned: usize,
    pending: Vec<ClipReq>,
    inflight: Vec<(Arc<AtomicBool>, f64)>,
    keep_secs: f64,
    origin_logged: bool,
}

impl Session {
    pub fn start(cfg: &Config, ffmpeg: &Path, enc: &Encoder, game_exe: &str) -> Result<Session, String> {
        let buffer_root = cfg.output_dir.join(".buffer");
        let started_ts = util::timestamp();
        let dir = util::unique_path(&buffer_root, &started_ts, "rec");
        std::fs::create_dir_all(&dir).map_err(|e| format!("No se pudo crear {}: {e}", dir.display()))?;
        crate::platform::hide_path(&buffer_root);
        let _ = std::fs::write(dir.join("session.txt"), format!("{game_exe}\n{}\n", cfg.mode.as_str()));

        let mut audio = None;
        let mut afmt = None;
        if cfg.audio {
            match AudioThread::start() {
                Ok((a, f)) => {
                    audio = Some(a);
                    afmt = Some(f);
                }
                Err(e) => log!("Audio no disponible, se graba sin sonido: {e}"),
            }
        }

        let mut args = sv(&["-hide_banner", "-loglevel", "warning", "-nostats", "-y", "-thread_queue_size", "512"]);
        args.extend(crate::platform::video_input(cfg));
        if let Some(f) = afmt {
            args.extend(sv(&["-thread_queue_size", "1024", "-f", f.ffmpeg_fmt()]));
            args.extend(["-ar".into(), f.rate.to_string(), "-ac".into(), f.channels.to_string()]);
            args.extend(sv(&["-use_wallclock_as_timestamps", "1"]));
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
        if afmt.is_some() {
            args.extend(sv(&[
                "-map", "1:a", "-af", "aresample=async=1000", "-c:a", "aac", "-b:a", "160k", "-ac", "2", "-shortest",
            ]));
        }
        args.extend(sv(&["-progress", "pipe:1", "-stats_period", "0.5"]));
        args.extend(sv(&[
            "-f", "segment", "-segment_time", "2", "-segment_format", "mpegts",
            "-segment_list", "list.csv", "-segment_list_type", "csv", "-reset_timestamps", "0",
            "seg_%06d.ts",
        ]));

        let mut child = util::hidden(Command::new(ffmpeg).args(&args))
            .current_dir(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("No se pudo iniciar ffmpeg: {e}"))?;
        let spawned = Instant::now();

        // Reloj: FFmpeg informa del tiempo grabado; origen = ahora - tiempo grabado.
        let origin = Arc::new(Mutex::new(None::<Instant>));
        if let Some(out) = child.stdout.take() {
            let origin = origin.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(v) = line.strip_prefix("out_time_us=") {
                        if let Ok(us) = v.trim().parse::<i64>() {
                            if us > 0 {
                                if let Some(c) = Instant::now().checked_sub(Duration::from_micros(us as u64)) {
                                    let mut o = origin.lock().unwrap();
                                    if o.map_or(true, |x| c < x) {
                                        *o = Some(c);
                                    }
                                }
                            }
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
                a.attach(stdin);
            }
        }

        log!("Grabando {game_exe} con {} en {}", enc.label, dir.display());
        Ok(Session {
            child,
            audio,
            origin,
            spawned,
            stderr_tail,
            dir,
            out_root: cfg.output_dir.clone(),
            game: util::friendly_game(game_exe),
            started_ts,
            mode: cfg.mode,
            clips_in_full: cfg.clips_in_full,
            segs: Vec::new(),
            csv_off: 0,
            csv_partial: String::new(),
            pruned: 0,
            pending: Vec::new(),
            inflight: Vec::new(),
            keep_secs: cfg.pre.max(cfg.manual_secs) as f64 + 10.0,
            origin_logged: false,
        })
    }

    pub fn game(&self) -> &str {
        &self.game
    }

    /// Pide un clip alrededor de `at`. Momentos cercanos se fusionan en un único clip.
    pub fn request_clip(&mut self, at: Instant, pre: f64, post: f64, label: &str, prio: u8, manual: bool) {
        if self.mode == Mode::Completa && !self.clips_in_full && !manual {
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

    fn origin(&mut self) -> Option<Instant> {
        let o = *self.origin.lock().unwrap();
        if let (Some(o), false) = (o, self.origin_logged) {
            self.origin_logged = true;
            log!("Reloj sincronizado: el vídeo empezó {} ms tras lanzar ffmpeg", secs_between(self.spawned, o) * 1000.0);
        }
        o
    }

    fn read_csv(&mut self) {
        let Ok(mut f) = std::fs::File::open(self.dir.join("list.csv")) else { return };
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
                if let (Ok(a), Ok(b)) = (a.parse(), b.parse()) {
                    self.segs.push(Seg { file: name.trim_matches('"').to_string(), start: a, end: b });
                }
            }
        }
    }

    fn flush(&mut self, jobs: &Sender<Job>, force: bool) {
        let origin = match self.origin() {
            Some(o) => o,
            None if force => self.spawned + Duration::from_millis(500),
            None => return,
        };
        let last_end = self.segs.last().map(|s| s.end).unwrap_or(0.0);
        let mut i = 0;
        while i < self.pending.len() {
            let e = secs_between(origin, self.pending[i].end);
            if force || last_end >= e {
                let p = self.pending.remove(i);
                let s = secs_between(origin, p.start).max(0.0);
                self.make_clip(jobs, s, e, &p.label);
            } else {
                i += 1;
            }
        }
    }

    fn make_clip(&mut self, jobs: &Sender<Job>, s: f64, e: f64, label: &str) {
        let files: Vec<String> = self.segs[self.pruned..]
            .iter()
            .filter(|g| g.end > s && g.start < e)
            .map(|g| g.file.clone())
            .collect();
        if files.is_empty() {
            log!("Clip '{label}' descartado: no hay vídeo grabado para ese momento");
            return;
        }
        let dest = self.out_root.join(&self.game).join("Clips");
        let stem = format!("{}_{}_{}", self.game, util::timestamp(), label);
        let done = Arc::new(AtomicBool::new(false));
        self.inflight.push((done.clone(), s));
        let _ = jobs.send(Job::Concat {
            dir: self.dir.clone(),
            files,
            out: util::unique_path(&dest, &stem, "mp4"),
            faststart: true,
            done: Some(done),
            notice: Some(format!("Clip guardado: {label}")),
        });
    }

    fn prune(&mut self) {
        if self.mode != Mode::Momentos {
            return;
        }
        let Some(origin) = self.origin() else { return };
        self.inflight.retain(|(d, _)| !d.load(Ordering::SeqCst));
        let mut cutoff = secs_between(origin, Instant::now()) - self.keep_secs;
        for p in &self.pending {
            cutoff = cutoff.min(secs_between(origin, p.start));
        }
        for (_, s) in &self.inflight {
            cutoff = cutoff.min(*s);
        }
        while self.pruned < self.segs.len() && self.segs[self.pruned].end < cutoff {
            let _ = std::fs::remove_file(self.dir.join(&self.segs[self.pruned].file));
            self.pruned += 1;
        }
    }

    /// Se llama periódicamente. Devuelve false si FFmpeg se ha detenido por su cuenta.
    pub fn tick(&mut self, jobs: &Sender<Job>) -> bool {
        self.read_csv();
        self.flush(jobs, false);
        self.prune();
        match self.child.try_wait() {
            Ok(Some(st)) => {
                let tail: Vec<String> = self.stderr_tail.lock().unwrap().iter().cloned().collect();
                log!("FFmpeg se detuvo ({st}). Últimos mensajes:\n{}", tail.join("\n"));
                false
            }
            _ => true,
        }
    }

    pub fn set_mode(&mut self, m: Mode) {
        self.mode = m;
        let _ = std::fs::write(self.dir.join("session.txt"), format!("{}\n{}\n", self.game, m.as_str()));
    }

    /// Detiene la grabación, guarda los clips pendientes y (modo completa) la partida entera.
    pub fn stop(mut self, jobs: &Sender<Job>) {
        if let Some(a) = self.audio.take() {
            a.stop();
        } else if let Some(mut stdin) = self.child.stdin.take() {
            let _ = stdin.write_all(b"q");
        }
        if util::wait_timeout(&mut self.child, Duration::from_secs(8)).is_none() {
            log!("FFmpeg no respondió; se forzó el cierre");
        }
        self.read_csv();
        self.flush(jobs, true);
        if self.mode == Mode::Completa && self.pruned < self.segs.len() {
            let files: Vec<String> = self.segs[self.pruned..].iter().map(|g| g.file.clone()).collect();
            let dest = self.out_root.join(&self.game).join("Partidas");
            let stem = format!("{}_{}", self.game, self.started_ts);
            let _ = jobs.send(Job::Concat {
                dir: self.dir.clone(),
                files,
                out: util::unique_path(&dest, &stem, "mp4"),
                faststart: false,
                done: None,
                notice: Some(format!("Partida guardada ({})", self.game)),
            });
        }
        let _ = jobs.send(Job::RemoveDir(self.dir.clone()));
        log!("Grabación detenida ({})", self.game);
    }
}
