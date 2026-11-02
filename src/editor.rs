// Editor: convierte un proyecto (trozos de vídeos con recorte, velocidad y texto, más formato,
// pistas de audio y calidad) en una sola orden de FFmpeg, y la ejecuta en segundo plano con
// progreso. Se codifica con la GPU (la misma familia que graba) y, si falla, con la CPU.
// Los originales nunca se tocan: el resultado va a <carpeta>/<juego>/Editados/.
use crate::json::Json;
use crate::util;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ------------------------------------------------------------------ Proyecto

#[derive(Clone, Debug, PartialEq)]
pub enum Format {
    Original,
    Vertical,
    VerticalBlur,
    Square,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Quality {
    High,
    P720,
    Discord,
}

#[derive(Clone, Debug)]
pub struct Segment {
    pub file: String,
    pub start: f64,
    pub end: f64,
    pub speed: f64,
    pub text: String,
    pub text_pos: String,
    /// Nombres de las pistas de audio del archivo (la 0 es la mezcla completa).
    pub tracks: Vec<String>,
}

impl Segment {
    pub fn out_dur(&self) -> f64 {
        (self.end - self.start).max(0.0) / self.speed
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    pub name: String,
    pub session: String,
    pub game: String,
    pub format: Format,
    pub focus: f64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub quality: Quality,
    pub fade: bool,
    pub muted: Vec<String>,
    pub segments: Vec<Segment>,
}

impl Project {
    pub fn total(&self) -> f64 {
        self.segments.iter().map(Segment::out_dur).sum()
    }

    pub fn from_json(j: &Json) -> Result<Project, String> {
        let s = |k: &str| j.get(k).and_then(Json::as_str).unwrap_or("").to_string();
        let n = |k: &str, d: f64| j.get(k).and_then(Json::as_f64).unwrap_or(d);
        let mut segments = Vec::new();
        for g in j.get("segments").map(Json::as_arr).unwrap_or(&[]) {
            let gs = |k: &str| g.get(k).and_then(Json::as_str).unwrap_or("").to_string();
            let gn = |k: &str, d: f64| g.get(k).and_then(Json::as_f64).unwrap_or(d);
            let seg = Segment {
                file: gs("file"),
                start: gn("in", 0.0).max(0.0),
                end: gn("out", 0.0),
                speed: [0.25, 0.5, 0.75, 1.0, 1.5, 2.0].into_iter().find(|v| (v - gn("speed", 1.0)).abs() < 1e-6).unwrap_or(1.0),
                text: gs("text").chars().take(120).collect(),
                text_pos: gs("text_pos"),
                tracks: g.get("tracks").map(Json::as_arr).unwrap_or(&[]).iter().filter_map(|t| t.as_str().map(String::from)).collect(),
            };
            if seg.file.is_empty() || seg.end - seg.start < 0.1 {
                return Err("Hay un trozo vacío o demasiado corto".into());
            }
            segments.push(seg);
        }
        if segments.is_empty() {
            return Err("El proyecto no tiene trozos".into());
        }
        if segments.len() > 60 {
            return Err("Demasiados trozos (máximo 60)".into());
        }
        Ok(Project {
            name: s("name"),
            session: s("session"),
            game: s("game"),
            format: match s("format").as_str() {
                "vertical" => Format::Vertical,
                "vertical_blur" => Format::VerticalBlur,
                "square" => Format::Square,
                _ => Format::Original,
            },
            focus: n("focus", 0.5).clamp(0.0, 1.0),
            width: (n("width", 1920.0) as u32).clamp(320, 7680),
            height: (n("height", 1080.0) as u32).clamp(240, 4320),
            fps: if n("fps", 60.0) <= 30.0 { 30 } else { 60 },
            quality: match s("quality").as_str() {
                "720p" => Quality::P720,
                "discord" => Quality::Discord,
                _ => Quality::High,
            },
            fade: j.get("fade").map_or(false, Json::as_bool),
            muted: j.get("muted").map(Json::as_arr).unwrap_or(&[]).iter().filter_map(|t| t.as_str().map(String::from)).collect(),
            segments,
        })
    }
}

// ------------------------------------------------------------------ Información de un archivo

#[derive(Clone, Debug, Default)]
pub struct MediaInfo {
    pub audio_streams: usize,
}

/// Cuántas pistas de audio tiene un vídeo (se lee de la salida de `ffmpeg -i`).
pub fn probe(ffmpeg: &Path, file: &Path) -> MediaInfo {
    let out = util::hidden(Command::new(ffmpeg).args(["-hide_banner", "-i"]).arg(file))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stderr).into_owned()).unwrap_or_default();
    MediaInfo { audio_streams: text.lines().filter(|l| l.trim_start().starts_with("Stream #") && l.contains("Audio:")).count() }
}

// ------------------------------------------------------------------ Construcción de la orden

fn even(x: f64) -> u32 {
    ((x / 2.0).round() as u32 * 2).max(2)
}

/// Tamaño final del vídeo.
pub fn out_size(p: &Project, small: bool) -> (u32, u32) {
    let short = if small { 720.0 } else { 1080.0 };
    match p.format {
        Format::Vertical | Format::VerticalBlur => (even(short), even(short * 16.0 / 9.0)),
        Format::Square => (even(short), even(short)),
        Format::Original => {
            let (w, h) = (p.width as f64, p.height as f64);
            if small && h > 720.0 {
                (even(w * 720.0 / h), 720)
            } else {
                (even(w), even(h))
            }
        }
    }
}

/// Velocidad de audio: atempo admite 0,5–2; 0,25 = dos veces 0,5.
fn atempo(speed: f64) -> String {
    if (speed - 1.0).abs() < 1e-6 {
        String::new()
    } else if speed < 0.5 {
        format!(",atempo=0.5,atempo={:.4}", speed / 0.5)
    } else {
        format!(",atempo={speed:.4}")
    }
}

pub struct Plan {
    pub args: Vec<String>,
    pub total: f64,
    pub size: (u32, u32),
    pub video_kbps: u32,
    /// Archivos de texto que hay que escribir antes de lanzar FFmpeg (nombre, contenido).
    pub text_files: Vec<(String, String)>,
}

/// Construye los argumentos de FFmpeg. `dir` es la carpeta de trabajo (allí van los textos y la fuente),
/// `inputs` las rutas absolutas de cada trozo y `audio` cuántas pistas de audio tiene cada una.
pub fn build(p: &Project, inputs: &[PathBuf], audio: &[usize], font: bool, family: &str, high_mbps: u32, out: &Path) -> Plan {
    let total = p.total();
    // Calidad → tamaño y tasa de bits.
    let (small, video_kbps) = match p.quality {
        Quality::High => (false, high_mbps.max(8) * 1000),
        Quality::P720 => (true, 6000),
        Quality::Discord => {
            // Meta: menos de 10 MB (límite gratuito de Discord), con margen y 128 kbps de audio.
            let bits = 10_000_000.0 * 8.0 * 0.90;
            let v = (bits / total.max(1.0) / 1000.0 - 128.0).clamp(250.0, 8000.0) as u32;
            (v < 4500, v)
        }
    };
    let (w, h) = out_size(p, small);
    let fps = p.fps;

    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-nostats"].iter().map(|s| s.to_string()).collect();
    for (seg, file) in p.segments.iter().zip(inputs) {
        args.extend(["-ss".into(), format!("{:.3}", seg.start), "-t".into(), format!("{:.3}", seg.end - seg.start), "-i".into()]);
        args.push(file.to_string_lossy().into_owned());
    }

    let mut f = String::new();
    let mut text_files = Vec::new();
    let mut cat = String::new();
    for (i, seg) in p.segments.iter().enumerate() {
        let d = seg.out_dur();
        // --- vídeo
        let base = format!("[{i}:v]setpts=(PTS-STARTPTS)/{sp:.4},fps={fps},trim=duration={d:.3}", sp = seg.speed);
        let fx = p.focus;
        match p.format {
            Format::Original => {
                f.push_str(&format!("{base},scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1[v{i}r];"));
            }
            Format::Vertical => {
                f.push_str(&format!("{base},crop=w='min(iw,ih*9/16)':h=ih:x='(iw-ow)*{fx:.3}':y=0,scale={w}:{h},setsar=1[v{i}r];"));
            }
            Format::Square => {
                f.push_str(&format!("{base},crop=w='min(iw,ih)':h='min(iw,ih)':x='(iw-ow)*{fx:.3}':y=(ih-oh)/2,scale={w}:{h},setsar=1[v{i}r];"));
            }
            Format::VerticalBlur => {
                f.push_str(&format!(
                    "{base},split[b{i}][f{i}];[b{i}]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},boxblur=24:2[bg{i}];\
                     [f{i}]scale={w}:-2[fg{i}];[bg{i}][fg{i}]overlay=0:(H-h)/2,setsar=1[v{i}r];"
                ));
            }
        }
        // texto
        let text = clean_text(&seg.text);
        if font && !text.is_empty() {
            let name = format!("texto{i}.txt");
            // Tamaño: 5,5 % del alto, o menos si el texto es largo (que quepa en el 92 % del ancho).
            let chars = text.chars().count().max(1) as f64;
            let size = (h as f64 * 0.055).min(w as f64 * 1.55 / chars).max(18.0).round();
            text_files.push((name.clone(), text.clone()));
            let y = match seg.text_pos.as_str() {
                "center" => "(h-text_h)/2",
                "bottom" => "h*0.86-text_h",
                _ => "h*0.07",
            };
            f.push_str(&format!(
                "[v{i}r]drawtext=fontfile=fuente.ttf:textfile={name}:fontsize={size}:fontcolor=white:borderw=5:bordercolor=black@0.85:x=(w-text_w)/2:y={y}[v{i}];"
            ));
        } else {
            f.push_str(&format!("[v{i}r]null[v{i}];"));
        }
        // --- audio: las pistas no silenciadas (o la mezcla completa si no se silenció nada)
        let n_audio = audio.get(i).copied().unwrap_or(0);
        let kept: Vec<usize> = if p.muted.is_empty() {
            if n_audio > 0 { vec![0] } else { vec![] }
        } else {
            (1..n_audio.min(seg.tracks.len().max(1)))
                .filter(|k| seg.tracks.get(*k).map_or(true, |name| !p.muted.contains(name)))
                .collect()
        };
        let tail = format!("{},aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,apad,atrim=duration={d:.3},asetpts=PTS-STARTPTS[a{i}];", atempo(seg.speed));
        match kept.len() {
            0 => f.push_str(&format!("anullsrc=r=48000:cl=stereo,atrim=duration={d:.3}[a{i}];")),
            1 => f.push_str(&format!("[{i}:a:{}]asetpts=PTS-STARTPTS{tail}", kept[0])),
            n => {
                for k in &kept {
                    f.push_str(&format!("[{i}:a:{k}]"));
                }
                f.push_str(&format!("amix=inputs={n}:normalize=0:duration=longest,asetpts=PTS-STARTPTS{tail}"));
            }
        }
        cat.push_str(&format!("[v{i}][a{i}]"));
    }
    let n = p.segments.len();
    f.push_str(&format!("{cat}concat=n={n}:v=1:a=1[vc][ac];"));
    let pix = if family == "cpu" { "yuv420p" } else { "nv12" };
    if p.fade && total > 1.5 {
        let fd = 0.5_f64.min(total / 4.0);
        let st = total - fd;
        f.push_str(&format!("[vc]fade=t=in:st=0:d={fd:.2},fade=t=out:st={st:.3}:d={fd:.2},format={pix}[vo];"));
        f.push_str(&format!("[ac]afade=t=in:st=0:d={fd:.2},afade=t=out:st={st:.3}:d={fd:.2}[ao]"));
    } else {
        f.push_str(&format!("[vc]format={pix}[vo];[ac]anull[ao]"));
    }
    args.extend(["-filter_complex".into(), f, "-map".into(), "[vo]".into(), "-map".into(), "[ao]".into()]);

    // --- codificación (siempre H.264: se reproduce en todas partes)
    let b = format!("{video_kbps}k");
    let (mx, bs) = if p.quality == Quality::Discord {
        (format!("{video_kbps}k"), format!("{video_kbps}k"))
    } else {
        (format!("{}k", video_kbps * 3 / 2), format!("{}k", video_kbps * 2))
    };
    let enc: Vec<&str> = match family {
        "nvenc" => vec!["-c:v", "h264_nvenc", "-preset", "p5", "-rc", "vbr"],
        "amf" => vec!["-c:v", "h264_amf", "-quality", "quality", "-rc", "vbr_peak"],
        "qsv" => vec!["-c:v", "h264_qsv", "-preset", "medium"],
        _ => vec!["-c:v", "libx264", "-preset", "veryfast"],
    };
    args.extend(enc.iter().map(|s| s.to_string()));
    args.extend(["-b:v".into(), b, "-maxrate".into(), mx, "-bufsize".into(), bs]);
    args.extend(["-g".into(), (fps * 2).to_string(), "-c:a".into(), "aac".into(), "-b:a".into(), "128k".into()]);
    args.extend(["-movflags".into(), "+faststart".into(), "-progress".into(), "pipe:1".into()]);
    args.push(out.to_string_lossy().into_owned());
    Plan { args, total, size: (w, h), video_kbps, text_files }
}

/// Texto apto para la fuente: una línea, sin emojis ni símbolos que la fuente no tiene.
pub fn clean_text(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|&c| (c as u32) < 0x2000 || ('\u{2010}'..='\u{2027}').contains(&c) || c == '€')
        .collect();
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ------------------------------------------------------------------ Trabajos de exportación

#[derive(Clone, Debug)]
pub enum State {
    Running(f64),
    Done { path: PathBuf, name: String, bytes: u64 },
    Failed(String),
    Cancelled,
}

struct Job {
    state: Mutex<State>,
    child: Mutex<Option<Child>>,
    cancel: AtomicBool,
}

static JOBS: Mutex<Option<HashMap<String, Arc<Job>>>> = Mutex::new(None);
static ENCODER: Mutex<(String, u32)> = Mutex::new((String::new(), 20));

/// La familia de codificador que funciona en este PC (la misma que la grabación) y la calidad alta.
pub fn set_encoder(label: &str, bitrate_mbps: u32) {
    let fam = if label.contains("nvenc") {
        "nvenc"
    } else if label.contains("amf") {
        "amf"
    } else if label.contains("qsv") {
        "qsv"
    } else {
        "cpu"
    };
    *ENCODER.lock().unwrap() = (fam.into(), bitrate_mbps);
}

pub fn status(id: &str) -> Option<State> {
    let jobs = JOBS.lock().unwrap();
    jobs.as_ref()?.get(id).map(|j| j.state.lock().unwrap().clone())
}

pub fn cancel(id: &str) {
    let job = JOBS.lock().unwrap().as_ref().and_then(|m| m.get(id).cloned());
    if let Some(j) = job {
        j.cancel.store(true, Ordering::SeqCst);
        if let Some(c) = j.child.lock().unwrap().as_mut() {
            let _ = c.kill();
        }
    }
}

/// Fuente para los textos (se copia a la carpeta de trabajo para no tener que escapar rutas).
fn font_file() -> Option<PathBuf> {
    if let Some(f) = std::env::var_os("CLIPLITE_FONT") {
        return Some(PathBuf::from(f));
    }
    let win = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let candidates = [
        win.join(r"Fonts\arialbd.ttf"),
        win.join(r"Fonts\segoeuib.ttf"),
        win.join(r"Fonts\arial.ttf"),
        PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
        PathBuf::from("/usr/share/fonts/truetype/google-fonts/Poppins-Bold.ttf"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

/// Lanza la exportación en segundo plano. Devuelve el identificador del trabajo.
pub fn start(ffmpeg: PathBuf, root: PathBuf, project: Project, inputs: Vec<PathBuf>) -> String {
    let id = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let job = Arc::new(Job { state: Mutex::new(State::Running(0.0)), child: Mutex::new(None), cancel: AtomicBool::new(false) });
    JOBS.lock().unwrap().get_or_insert_with(HashMap::new).insert(id.clone(), job.clone());
    let jid = id.clone();
    std::thread::Builder::new()
        .name("editor".into())
        .spawn(move || {
            let result = run(&ffmpeg, &root, &project, &inputs, &job, &jid);
            let st = match result {
                Ok((path, bytes)) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    log!("Vídeo editado: {} ({})", path.display(), util::human_size(bytes));
                    let rel = path.strip_prefix(&root).map(|r| r.to_path_buf()).unwrap_or(path.clone());
                    let _ = crate::library::add_edited(&root, &project.session, &rel, project.total());
                    State::Done { path, name, bytes }
                }
                Err(_) if job.cancel.load(Ordering::SeqCst) => State::Cancelled,
                Err(e) => {
                    log!("Error al exportar: {e}");
                    State::Failed(e)
                }
            };
            *job.state.lock().unwrap() = st;
        })
        .ok();
    id
}

fn run(ffmpeg: &Path, root: &Path, p: &Project, inputs: &[PathBuf], job: &Job, id: &str) -> Result<(PathBuf, u64), String> {
    let work = root.join(".cliplite").join("tmp").join(format!("edit_{id}"));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let font = font_file().and_then(|f| std::fs::copy(f, work.join("fuente.ttf")).ok()).is_some();
    if !font && p.segments.iter().any(|s| !s.text.trim().is_empty()) {
        log!("Editor: no se encontró una fuente; se exporta sin textos");
    }
    let audio: Vec<usize> = inputs.iter().map(|f| probe(ffmpeg, f).audio_streams).collect();
    let game = if p.game.is_empty() { "Editados".to_string() } else { util::sanitize(&p.game) };
    let dest = if p.game.is_empty() { root.join("Editados") } else { root.join(&game).join("Editados") };
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    let stem = {
        let n = util::sanitize(p.name.trim());
        if n == "Juego" || p.name.trim().is_empty() { format!("{game}_{}_editado", util::timestamp()) } else { n }
    };
    let out = util::unique_path(&dest, &stem, "mp4");
    let part = work.join("salida.mp4");

    let (family, mbps) = ENCODER.lock().unwrap().clone();
    let family = if family.is_empty() { "cpu".to_string() } else { family };
    let mut families = vec![family.clone()];
    if family != "cpu" {
        families.push("cpu".into());
    }
    let mut last_err = String::new();
    for fam in families {
        if job.cancel.load(Ordering::SeqCst) {
            break;
        }
        let plan = build(p, inputs, &audio, font, &fam, mbps, &part);
        for (name, text) in &plan.text_files {
            std::fs::write(work.join(name), text).map_err(|e| e.to_string())?;
        }
        log!("Editor: exportando {:.1} s a {}x{} ({} kbps) con {fam}", plan.total, plan.size.0, plan.size.1, plan.video_kbps);
        match encode(ffmpeg, &work, &plan, job) {
            Ok(()) => {
                std::fs::rename(&part, &out).or_else(|_| std::fs::copy(&part, &out).map(|_| ())).map_err(|e| e.to_string())?;
                let _ = std::fs::remove_dir_all(&work);
                let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
                return Ok((out, bytes));
            }
            Err(e) => {
                log!("Editor: falló con {fam}: {e}");
                last_err = e;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    Err(last_err)
}

fn encode(ffmpeg: &Path, work: &Path, plan: &Plan, job: &Job) -> Result<(), String> {
    let mut child = util::hidden(Command::new(ffmpeg).args(&plan.args))
        .current_dir(work)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    *job.child.lock().unwrap() = Some(child);
    let err_text = Arc::new(Mutex::new(String::new()));
    if let Some(e) = stderr {
        let et = err_text.clone();
        std::thread::spawn(move || {
            for l in BufReader::new(e).lines().map_while(Result::ok) {
                let mut s = et.lock().unwrap();
                if s.len() < 4000 {
                    s.push_str(&l);
                    s.push('\n');
                }
            }
        });
    }
    if let Some(o) = stdout {
        for l in BufReader::new(o).lines().map_while(Result::ok) {
            if let Some(v) = l.strip_prefix("out_time_us=") {
                if let Ok(us) = v.trim().parse::<f64>() {
                    let pct = (us / 1e6 / plan.total.max(0.1)).clamp(0.0, 0.99);
                    *job.state.lock().unwrap() = State::Running(pct);
                }
            }
        }
    }
    let status = loop {
        let mut guard = job.child.lock().unwrap();
        match guard.as_mut().map(|c| c.try_wait()) {
            Some(Ok(Some(st))) => break Some(st),
            Some(Ok(None)) => {}
            _ => break None,
        }
        drop(guard);
        std::thread::sleep(Duration::from_millis(50));
    };
    *job.child.lock().unwrap() = None;
    std::thread::sleep(Duration::from_millis(50));
    match status {
        Some(st) if st.success() => Ok(()),
        _ if job.cancel.load(Ordering::SeqCst) => Err("cancelado".into()),
        _ => Err(err_text.lock().unwrap().trim().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(json: &str) -> Project {
        Project::from_json(&crate::json::parse(json).unwrap()).unwrap()
    }

    #[test]
    fn parse_and_sizes() {
        let p = project(r#"{"format":"vertical","quality":"alta","width":2560,"height":1440,"segments":[{"file":"a.mp4","in":1,"out":5,"speed":0.5},{"file":"b.mp4","in":0,"out":3,"speed":7}]}"#);
        assert_eq!(p.segments[1].speed, 1.0, "velocidad no permitida → 1×");
        assert!((p.total() - (8.0 + 3.0)).abs() < 1e-9);
        assert_eq!(out_size(&p, false), (1080, 1920));
        assert_eq!(out_size(&p, true), (720, 1280));
        let mut q = p.clone();
        q.format = Format::Original;
        assert_eq!(out_size(&q, false), (2560, 1440));
        assert_eq!(out_size(&q, true), (1280, 720));
        q.format = Format::Square;
        assert_eq!(out_size(&q, false), (1080, 1080));
        assert!(Project::from_json(&crate::json::parse(r#"{"segments":[]}"#).unwrap()).is_err());
        assert!(Project::from_json(&crate::json::parse(r#"{"segments":[{"file":"a","in":2,"out":2.05}]}"#).unwrap()).is_err());
    }

    #[test]
    fn text_cleanup() {
        assert_eq!(clean_text("  ¡Doble kill! 🔥\n\"épica\" — ñ €  "), "¡Doble kill! \"épica\" — ñ €");
        assert_eq!(clean_text("🔥🔥"), "");
    }

    #[test]
    fn discord_bitrate_fits() {
        let p = project(r#"{"quality":"discord","segments":[{"file":"a","in":0,"out":60}]}"#);
        let plan = build(&p, &[PathBuf::from("a")], &[1], false, "cpu", 20, Path::new("o.mp4"));
        let mb = (plan.video_kbps as f64 + 128.0) * 1000.0 * 60.0 / 8.0 / 1e6;
        assert!(mb < 10.0, "{mb} MB");
        assert_eq!(plan.size.1, 720, "a baja tasa se exporta en 720p");
    }

    #[test]
    fn audio_track_selection() {
        let p = project(r#"{"muted":["Discord"],"segments":[{"file":"a","in":0,"out":5,"tracks":["Todo","Juego","Discord","Micrófono"]}]}"#);
        let plan = build(&p, &[PathBuf::from("a")], &[4], false, "cpu", 20, Path::new("o.mp4"));
        let f = &plan.args[plan.args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(f.contains("[0:a:1][0:a:3]amix=inputs=2"), "{f}");
        assert!(!f.contains("[0:a:2]") && !f.contains("[0:a:0]"));
        // Sin pistas silenciadas → mezcla completa; sin audio → silencio.
        let p = project(r#"{"segments":[{"file":"a","in":0,"out":5}]}"#);
        let plan = build(&p, &[PathBuf::from("a")], &[1], false, "cpu", 20, Path::new("o.mp4"));
        assert!(plan.args.iter().any(|a| a.contains("[0:a:0]")));
        let plan = build(&p, &[PathBuf::from("a")], &[0], false, "cpu", 20, Path::new("o.mp4"));
        assert!(plan.args.iter().any(|a| a.contains("anullsrc")));
    }
}
