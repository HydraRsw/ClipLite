// Servidor local del reproductor (solo 127.0.0.1).
// Sirve la página y los vídeos (con soporte de Range para poder saltar en el vídeo) y ofrece
// tres acciones que un navegador no puede hacer solo:
//   /api/mix     → versión del vídeo con solo las pistas de audio elegidas (vista previa)
//   /api/export  → guarda esa versión como un vídeo nuevo junto al original
//   /api/reveal  → muestra el archivo en el Explorador
// Seguridad: escucha solo en 127.0.0.1, exige un token aleatorio en las acciones y rechaza
// peticiones cuyo Host no sea 127.0.0.1/localhost (evita ataques de "DNS rebinding").
// Sin peticiones, el hilo está bloqueado en accept(): no consume CPU.
use crate::json::Json;
use crate::util;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

struct Srv {
    port: u16,
    token: String,
    root: Mutex<PathBuf>,
    ffmpeg: PathBuf,
}

static SRV: OnceLock<Srv> = OnceLock::new();

fn random_hex() -> String {
    let mut out = String::new();
    for _ in 0..2 {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// Arranca el servidor en un puerto fijo (para que los favoritos del reproductor se conserven).
pub fn start(root: PathBuf, ffmpeg: PathBuf) {
    for port in 47380..47400u16 {
        let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) else { continue };
        let _ = std::fs::remove_dir_all(root.join(".cliplite").join("tmp"));
        let _ = SRV.set(Srv { port, token: random_hex(), root: Mutex::new(root), ffmpeg });
        std::thread::Builder::new()
            .name("servidor".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    std::thread::spawn(move || {
                        let _ = handle(stream);
                    });
                }
            })
            .ok();
        log!("Reproductor disponible en http://127.0.0.1:{port}/");
        return;
    }
    log!("No se pudo abrir el servidor del reproductor (puertos ocupados)");
}

pub fn set_root(root: PathBuf) {
    if let Some(s) = SRV.get() {
        *s.root.lock().unwrap() = root;
    }
}

/// URL del reproductor, si el servidor está funcionando.
pub fn base_url() -> Option<String> {
    SRV.get().map(|s| format!("http://127.0.0.1:{}/?t={}", s.port, s.token))
}

// ------------------------------------------------------------------ HTTP mínimo

fn decode(s: &str, plus_space: bool) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' if plus_space => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query(q: &str, key: &str) -> Option<String> {
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == key).then(|| decode(v, true))
    })
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    s.write_all(body)
}

fn json(s: &mut TcpStream, j: Json) -> std::io::Result<()> {
    respond(s, "200 OK", "application/json; charset=utf-8", j.to_text().as_bytes())
}

fn error(s: &mut TcpStream, msg: &str) -> std::io::Result<()> {
    json(s, Json::Obj(vec![("error".into(), Json::str(msg))]))
}

fn handle(mut stream: TcpStream) -> std::io::Result<()> {
    let srv = SRV.get().expect("servidor");
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let (mut host, mut range) = (String::new(), None::<String>);
    let mut content_len: usize = 0;
    let mut total = 0;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        total += h.len();
        if total > 16 * 1024 {
            return respond(&mut stream, "431 Request Header Fields Too Large", "text/plain", b"");
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "host" => host = v.trim().to_string(),
                "range" => range = Some(v.trim().to_string()),
                "content-length" => content_len = v.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    let ok_hosts = [format!("127.0.0.1:{}", srv.port), format!("localhost:{}", srv.port)];
    if !ok_hosts.contains(&host) {
        return respond(&mut stream, "403 Forbidden", "text/plain", b"");
    }
    let (path, q) = target.split_once('?').unwrap_or((target, ""));
    let mut body = Vec::new();
    if method == "POST" && path == "/api/edit" {
        if content_len > 1_000_000 {
            return respond(&mut stream, "413 Payload Too Large", "text/plain", b"");
        }
        body.resize(content_len, 0);
        reader.read_exact(&mut body)?;
    } else if method != "GET" && method != "HEAD" {
        return respond(&mut stream, "405 Method Not Allowed", "text/plain", b"");
    }
    let root = srv.root.lock().unwrap().clone();

    if path == "/" {
        let html = crate::library::ensure_html(&root);
        let body = std::fs::read(html).unwrap_or_default();
        return respond(&mut stream, "200 OK", "text/html; charset=utf-8", &body);
    }
    if let Some(api) = path.strip_prefix("/api/") {
        if query(q, "t").as_deref() != Some(srv.token.as_str()) {
            return respond(&mut stream, "403 Forbidden", "text/plain", b"");
        }
        let rel = query(q, "file").unwrap_or_default();
        let freed_json = |f: crate::library::Freed| {
            Json::Obj(vec![
                ("files".into(), Json::Num(f.files as f64)),
                ("bytes".into(), Json::Num(f.bytes as f64)),
                ("text".into(), Json::str(&f.describe())),
            ])
        };
        // Acciones que no necesitan un archivo existente.
        match api {
            "edit" => return edit_start(&mut stream, srv, &root, &body),
            "edit_status" => {
                let id = query(q, "id").unwrap_or_default();
                let j = match crate::editor::status(&id) {
                    None => Json::Obj(vec![("state".into(), Json::str("unknown"))]),
                    Some(crate::editor::State::Running(p)) => {
                        Json::Obj(vec![("state".into(), Json::str("running")), ("progress".into(), Json::Num(p))])
                    }
                    Some(crate::editor::State::Done { path, name, bytes }) => Json::Obj(vec![
                        ("state".into(), Json::str("done")),
                        ("name".into(), Json::str(&name)),
                        ("file".into(), Json::str(&rel_path(&root, &path))),
                        ("size".into(), Json::str(&util::human_size(bytes))),
                    ]),
                    Some(crate::editor::State::Failed(e)) => {
                        Json::Obj(vec![("state".into(), Json::str("failed")), ("error".into(), Json::str(&e))])
                    }
                    Some(crate::editor::State::Cancelled) => Json::Obj(vec![("state".into(), Json::str("cancelled"))]),
                };
                return json(&mut stream, j);
            }
            "edit_cancel" => {
                crate::editor::cancel(&query(q, "id").unwrap_or_default());
                return json(&mut stream, Json::Obj(vec![("ok".into(), Json::Bool(true))]));
            }
            "delete_session" => {
                let id = query(q, "id").unwrap_or_default();
                let f = crate::library::delete_session(&root, &id, true);
                return json(&mut stream, freed_json(f));
            }
            "cleanup" => {
                let days: u32 = query(q, "days").and_then(|d| d.parse().ok()).unwrap_or(0);
                if days == 0 {
                    return error(&mut stream, "Indica los días");
                }
                let f = crate::library::cleanup(&root, days, 0.0, true);
                return json(&mut stream, freed_json(f));
            }
            "fav" => {
                // Validar que la ruta sea relativa y sin "..".
                if rel.is_empty() || rel.split(['/', '\\']).any(|c| c == "..") {
                    return error(&mut stream, "Ruta inválida");
                }
                let on = query(q, "on").as_deref() == Some("1");
                return match crate::library::set_fav(&root, &rel, on) {
                    Ok(()) => json(&mut stream, Json::Obj(vec![("ok".into(), Json::Bool(true))])),
                    Err(e) => error(&mut stream, &e.to_string()),
                };
            }
            _ => {}
        }
        let Some(src) = safe_path(&root, &rel) else { return error(&mut stream, "Archivo no encontrado") };
        if api == "delete" {
            let f = crate::library::delete_file(&root, &rel, true);
            return json(&mut stream, freed_json(f));
        }
        let keep: Vec<usize> = query(q, "keep")
            .unwrap_or_default()
            .split(',')
            .filter_map(|x| x.parse().ok())
            .filter(|&k| k > 0 && k < 32)
            .collect();
        return match api {
            "mix" => match mix_preview(srv, &root, &src, &keep) {
                Ok(p) => {
                    let url = format!("/{}", rel_url(&root, &p));
                    json(&mut stream, Json::Obj(vec![("url".into(), Json::str(&url))]))
                }
                Err(e) => error(&mut stream, &e),
            },
            "export" => {
                let suffix = util::sanitize(&query(q, "suffix").unwrap_or_else(|| "editado".into()));
                match export(srv, &root, &src, &keep, &suffix) {
                    Ok(p) => json(
                        &mut stream,
                        Json::Obj(vec![
                            ("path".into(), Json::str(&p.to_string_lossy())),
                            ("name".into(), Json::str(&p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())),
                        ]),
                    ),
                    Err(e) => error(&mut stream, &e),
                }
            }
            "reveal" => {
                crate::platform::reveal(&src);
                json(&mut stream, Json::Obj(vec![("ok".into(), Json::Bool(true))]))
            }
            _ => respond(&mut stream, "404 Not Found", "text/plain", b""),
        };
    }
    let rel = decode(&path[1..], false);
    match safe_path(&root, &rel) {
        Some(p) => serve_file(&mut stream, &p, range.as_deref(), method == "HEAD"),
        None => respond(&mut stream, "404 Not Found", "text/plain", b""),
    }
}

/// Resuelve una ruta relativa dentro de la carpeta de vídeos (nunca fuera de ella).
fn safe_path(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.split(['/', '\\']).any(|c| c == "..") {
        return None;
    }
    let p = root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    let canon_root = root.canonicalize().ok()?;
    let canon = p.canonicalize().ok()?;
    (canon.starts_with(&canon_root) && canon.is_file()).then_some(canon)
}

/// Ruta relativa con '/' (sin codificar), como la guarda la biblioteca.
fn rel_path(root: &Path, p: &Path) -> String {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let r = p.strip_prefix(&root).unwrap_or(&p);
    r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

fn edit_start(stream: &mut TcpStream, srv: &Srv, root: &Path, body: &[u8]) -> std::io::Result<()> {
    let Some(j) = crate::json::parse(&String::from_utf8_lossy(body)) else { return error(stream, "Proyecto inválido") };
    let project = match crate::editor::Project::from_json(&j) {
        Ok(p) => p,
        Err(e) => return error(stream, &e),
    };
    let mut inputs = Vec::new();
    for seg in &project.segments {
        match safe_path(root, &seg.file) {
            Some(p) => inputs.push(p),
            None => return error(stream, &format!("No se encuentra el vídeo {}", seg.file)),
        }
    }
    let id = crate::editor::start(srv.ffmpeg.clone(), root.to_path_buf(), project, inputs);
    json(stream, Json::Obj(vec![("id".into(), Json::str(&id))]))
}

fn rel_url(root: &Path, p: &Path) -> String {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let r = p.strip_prefix(&root).unwrap_or(p);
    r.components().map(|c| util::url_encode(&c.as_os_str().to_string_lossy())).collect::<Vec<_>>().join("/")
}

fn mime(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp4") => "video/mp4",
        Some("m4a") => "audio/mp4",
        Some("html") => "text/html; charset=utf-8",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

fn serve_file(s: &mut TcpStream, p: &Path, range: Option<&str>, head: bool) -> std::io::Result<()> {
    let mut f = std::fs::File::open(p)?;
    let len = f.metadata()?.len();
    let (start, end, partial) = match range.and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.split_once('-')) {
        Some((a, b)) if len > 0 => {
            let (start, end) = if a.is_empty() {
                let n: u64 = b.parse().unwrap_or(0).min(len);
                (len - n, len - 1)
            } else {
                let a: u64 = a.parse().unwrap_or(0);
                let e = b.parse::<u64>().unwrap_or(len - 1).min(len - 1);
                (a, e)
            };
            if start > end || start >= len {
                write!(s, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
                return Ok(());
            }
            (start, end, true)
        }
        _ => (0, len.saturating_sub(1), false),
    };
    let n = if len == 0 { 0 } else { end - start + 1 };
    if partial {
        write!(
            s,
            "HTTP/1.1 206 Partial Content\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{len}\r\nContent-Length: {n}\r\nConnection: close\r\n\r\n",
            mime(p)
        )?;
    } else {
        write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Length: {n}\r\nConnection: close\r\n\r\n",
            mime(p)
        )?;
    }
    if head || n == 0 {
        return Ok(());
    }
    f.seek(SeekFrom::Start(start))?;
    let mut left = n;
    let mut buf = vec![0u8; 256 * 1024];
    while left > 0 {
        let want = left.min(buf.len() as u64) as usize;
        let k = f.read(&mut buf[..want])?;
        if k == 0 {
            break;
        }
        s.write_all(&buf[..k])?;
        left -= k as u64;
    }
    Ok(())
}

// ------------------------------------------------------------------ Mezcla de pistas

/// FFmpeg: copia el vídeo sin tocarlo y deja solo las pistas de audio indicadas (mezcladas).
fn run_mix(ffmpeg: &Path, src: &Path, keep: &[usize], out: &Path) -> Result<(), String> {
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-i"].iter().map(|s| s.to_string()).collect();
    args.push(src.to_string_lossy().into_owned());
    args.extend(["-map".into(), "0:v".into(), "-c:v".into(), "copy".into()]);
    match keep.len() {
        0 => args.push("-an".into()),
        1 => args.extend(["-map".into(), format!("0:a:{}", keep[0]), "-c:a".into(), "copy".into()]),
        n => {
            let inputs: String = keep.iter().map(|k| format!("[0:a:{k}]")).collect();
            args.extend([
                "-filter_complex".into(),
                format!("{inputs}amix=inputs={n}:normalize=0[a]"),
                "-map".into(),
                "[a]".into(),
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                "192k".into(),
            ]);
        }
    }
    args.extend(["-movflags".into(), "+faststart".into()]);
    args.push(out.to_string_lossy().into_owned());
    let o = util::hidden(Command::new(ffmpeg).args(&args))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
        log!("Error al mezclar pistas: {msg}");
        Err(msg)
    }
}

fn mix_preview(srv: &Srv, root: &Path, src: &Path, keep: &[usize]) -> Result<PathBuf, String> {
    let mut h: u64 = 0xcbf29ce484222325;
    let mtime = std::fs::metadata(src).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok());
    for b in format!("{}|{:?}|{:?}", src.display(), keep, mtime).bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    let dir = root.join(".cliplite").join("tmp");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join(format!("{h:016x}.mp4"));
    if !out.is_file() {
        let part = dir.join(format!("{h:016x}.part.mp4"));
        run_mix(&srv.ffmpeg, src, keep, &part)?;
        std::fs::rename(&part, &out).map_err(|e| e.to_string())?;
    }
    Ok(out.canonicalize().unwrap_or(out))
}

fn export(srv: &Srv, root: &Path, src: &Path, keep: &[usize], suffix: &str) -> Result<PathBuf, String> {
    let preview = mix_preview(srv, root, src, keep)?;
    let dir = src.parent().ok_or("ruta inválida")?;
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "clip".into());
    let dest = util::unique_path(dir, &format!("{stem}_{suffix}"), "mp4");
    std::fs::copy(&preview, &dest).map_err(|e| e.to_string())?;
    log!("Copia guardada: {}", dest.display());
    Ok(dest)
}
