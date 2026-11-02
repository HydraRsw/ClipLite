// Biblioteca de sesiones para el reproductor.
// Los datos viven en <carpeta>/.cliplite/biblioteca.json y se incrustan en <carpeta>/ClipLite.html,
// una página autónoma que se abre como ventana de aplicación al terminar cada partida.
use crate::json::{self, Json};
use crate::recorder::LibEntry;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// La biblioteca se modifica desde varios hilos (guardado, servidor, limpieza): uno a la vez.
static LOCK: Mutex<()> = Mutex::new(());

const TEMPLATE: &str = include_str!("player.html");
const MARKER: &str = "__CLIPLITE_DATA__";
const MAX_SESSIONS: usize = 500;

fn data_path(out_root: &Path) -> PathBuf {
    out_root.join(".cliplite").join("biblioteca.json")
}

pub fn html_path(out_root: &Path) -> PathBuf {
    out_root.join("ClipLite.html")
}

/// Ruta relativa a la carpeta de salida con '/' (para usarla desde la página).
fn rel(out_root: &Path, p: &Path) -> String {
    let r = p.strip_prefix(out_root).unwrap_or(p);
    r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

fn num(n: f64) -> Json {
    Json::Num((n * 1000.0).round() / 1000.0)
}

fn load(out_root: &Path) -> Vec<Json> {
    std::fs::read_to_string(data_path(out_root))
        .ok()
        .and_then(|s| json::parse(&s))
        .map(|j| j.as_arr().to_vec())
        .unwrap_or_default()
}

/// Añade (o reemplaza) una sesión y regenera la página. Devuelve (página, id de sesión).
pub fn add(out_root: &Path, e: LibEntry) -> std::io::Result<(PathBuf, String)> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let id = e.id.clone();
    let entry = Json::Obj(vec![
        ("id".into(), Json::str(&e.id)),
        ("game".into(), Json::str(&e.game)),
        ("date".into(), Json::str(&e.date)),
        ("duration".into(), num(e.duration)),
        ("match".into(), e.match_file.as_ref().map(|m| Json::str(&rel(out_root, m))).unwrap_or(Json::Null)),
        (
            "clips".into(),
            Json::Arr(
                e.clips
                    .iter()
                    .map(|c| {
                        Json::Obj(vec![
                            ("file".into(), Json::str(&rel(out_root, &c.file))),
                            ("label".into(), Json::str(&c.label)),
                            ("t".into(), num(c.t)),
                            ("dur".into(), num(c.dur)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("tracks".into(), Json::Arr(e.tracks.iter().map(|t| Json::str(t)).collect())),
        (
            "events".into(),
            Json::Arr(
                e.events
                    .iter()
                    .map(|(l, t)| Json::Obj(vec![("label".into(), Json::str(l)), ("t".into(), num(*t))]))
                    .collect(),
            ),
        ),
    ]);
    let mut list = load(out_root);
    list.retain(|x| x.get("id").and_then(Json::as_str) != Some(id.as_str()));
    list.push(entry);
    if list.len() > MAX_SESSIONS {
        let extra = list.len() - MAX_SESSIONS;
        list.drain(..extra);
    }
    save(out_root, &list)?;
    let html = write_html(out_root, &list)?;
    Ok((html, id))
}

fn save(out_root: &Path, list: &[Json]) -> std::io::Result<()> {
    let dp = data_path(out_root);
    if let Some(d) = dp.parent() {
        std::fs::create_dir_all(d)?;
        crate::platform::hide_path(d);
    }
    // Escritura atómica: primero a un temporal y luego se renombra.
    let tmp = dp.with_extension("json.tmp");
    std::fs::write(&tmp, Json::Arr(list.to_vec()).to_text())?;
    std::fs::rename(&tmp, &dp)
}

fn file_size(out_root: &Path, rel: Option<&str>) -> u64 {
    rel.and_then(|r| std::fs::metadata(out_root.join(r)).ok()).map_or(0, |m| m.len())
}

/// Añade un vídeo editado a su partida (o a una nueva entrada "Editados" si no hay partida).
pub fn add_edited(out_root: &Path, session: &str, rel_file: &Path, dur: f64) -> std::io::Result<()> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut list = load(out_root);
    let clip = Json::Obj(vec![
        ("file".into(), Json::str(&rel(out_root, &out_root.join(rel_file)))),
        ("label".into(), Json::str("editado")),
        ("t".into(), num(0.0)),
        ("dur".into(), num(dur)),
        ("edited".into(), Json::Bool(true)),
    ]);
    let target = list.iter_mut().find(|s| s.get("id").and_then(Json::as_str) == Some(session));
    match target {
        Some(s) => {
            if let Some(Json::Arr(clips)) = s.get_mut("clips") {
                clips.push(clip);
            }
        }
        None => {
            let ts = crate::util::timestamp();
            list.push(Json::Obj(vec![
                ("id".into(), Json::str(&format!("Editados_{ts}"))),
                ("game".into(), Json::str("Editados")),
                ("date".into(), Json::str(&ts)),
                ("duration".into(), num(dur)),
                ("match".into(), Json::Null),
                ("clips".into(), Json::Arr(vec![clip])),
                ("tracks".into(), Json::Arr(vec![])),
                ("events".into(), Json::Arr(vec![])),
            ]));
        }
    }
    save(out_root, &list)?;
    write_html(out_root, &list).map(|_| ())
}

// ------------------------------------------------------------------ Favoritos y borrado

/// Marca o desmarca un clip como favorito (los favoritos nunca se borran automáticamente).
pub fn set_fav(out_root: &Path, rel: &str, on: bool) -> std::io::Result<()> {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut list = load(out_root);
    for s in list.iter_mut() {
        if let Some(Json::Arr(clips)) = s.get_mut("clips") {
            for c in clips.iter_mut() {
                if c.get("file").and_then(Json::as_str) == Some(rel) {
                    c.set("fav", Json::Bool(on));
                }
            }
        }
    }
    save(out_root, &list)
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Freed {
    pub files: usize,
    pub bytes: u64,
}

impl Freed {
    pub fn describe(&self) -> String {
        format!("{} vídeo{} ({})", self.files, if self.files == 1 { "" } else { "s" }, crate::util::human_size(self.bytes))
    }
}

/// Borra un archivo de vídeo de la carpeta: a la Papelera (`recycle`) o definitivamente.
fn remove(out_root: &Path, rel: &str, recycle: bool, freed: &mut Freed) -> bool {
    let p = out_root.join(rel);
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    if !p.is_file() {
        return true; // ya no existe: se considera borrado
    }
    let ok = if recycle { crate::platform::trash(&p).is_ok() } else { std::fs::remove_file(&p).is_ok() };
    if ok {
        freed.files += 1;
        freed.bytes += size;
    } else {
        log!("No se pudo borrar {}", p.display());
    }
    ok
}

/// Borra en una sesión: la partida completa (si `with_match`) y los clips que cumplan `pick`.
/// Los favoritos nunca se tocan. Devuelve si la sesión quedó vacía.
fn prune_session(out_root: &Path, s: &mut Json, with_match: bool, recycle: bool, freed: &mut Freed, pick: &dyn Fn(&Json) -> bool) -> bool {
    if with_match {
        if let Some(m) = s.get("match").and_then(Json::as_str).map(String::from) {
            if remove(out_root, &m, recycle, freed) {
                s.set("match", Json::Null);
            }
        }
    }
    if let Some(Json::Arr(clips)) = s.get_mut("clips") {
        clips.retain(|c| {
            let fav = c.get("fav").map_or(false, Json::as_bool);
            let Some(f) = c.get("file").and_then(Json::as_str) else { return false };
            if fav || !pick(c) {
                return true;
            }
            !remove(out_root, f, recycle, freed)
        });
    }
    let has_match = !matches!(s.get("match"), Some(Json::Null) | None);
    let has_clips = s.get("clips").map_or(false, |c| !c.as_arr().is_empty());
    !has_match && !has_clips
}

/// Borra un clip o una partida concreta (desde el reproductor) y la quita de la biblioteca.
pub fn delete_file(out_root: &Path, rel: &str, recycle: bool) -> Freed {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut freed = Freed::default();
    let mut list = load(out_root);
    let mut found = false;
    list.retain_mut(|s| {
        let is_match = s.get("match").and_then(Json::as_str) == Some(rel);
        let has_clip = s.get("clips").map_or(false, |c| c.as_arr().iter().any(|c| c.get("file").and_then(Json::as_str) == Some(rel)));
        if !is_match && !has_clip {
            return true;
        }
        found = true;
        if let Some(Json::Arr(clips)) = s.get_mut("clips") {
            // Borrar a mano un favorito sí está permitido: es una decisión explícita.
            for c in clips.iter_mut() {
                if c.get("file").and_then(Json::as_str) == Some(rel) {
                    c.set("fav", Json::Bool(false));
                }
            }
        }
        !prune_session(out_root, s, is_match, recycle, &mut freed, &|c| c.get("file").and_then(Json::as_str) == Some(rel))
    });
    if !found {
        remove(out_root, rel, recycle, &mut freed);
    }
    let _ = save(out_root, &list);
    let _ = write_html(out_root, &list);
    freed
}

/// Borra una partida entera con sus clips, salvo los clips favoritos.
pub fn delete_session(out_root: &Path, id: &str, recycle: bool) -> Freed {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut freed = Freed::default();
    let mut list = load(out_root);
    list.retain_mut(|s| {
        if s.get("id").and_then(Json::as_str) != Some(id) {
            return true;
        }
        !prune_session(out_root, s, true, recycle, &mut freed, &|_| true)
    });
    let _ = save(out_root, &list);
    let _ = write_html(out_root, &list);
    freed
}

/// Limpieza de vídeos viejos:
///  * `days` > 0: borra lo de las partidas con más de `days` días.
///  * `limit_gb` > 0: si los vídeos ocupan más, borra empezando por las partidas más antiguas.
/// Nunca toca los clips favoritos.
pub fn cleanup(out_root: &Path, days: u32, limit_gb: f64, recycle: bool) -> Freed {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut freed = Freed::default();
    if days == 0 && limit_gb <= 0.0 {
        return freed;
    }
    let mut list = load(out_root);
    list.sort_by(|a, b| a.get("date").and_then(Json::as_str).cmp(&b.get("date").and_then(Json::as_str)));

    if days > 0 {
        for s in list.iter_mut() {
            let old = s.get("date").and_then(Json::as_str).and_then(crate::util::age_days).map_or(false, |a| a > days as i64);
            if old {
                prune_session(out_root, s, true, recycle, &mut freed, &|_| true);
            }
        }
    }
    if limit_gb > 0.0 {
        let limit = (limit_gb * 1e9) as u64;
        let size_of = |s: &Json| -> u64 {
            file_size(out_root, s.get("match").and_then(Json::as_str))
                + s.get("clips").map_or(0, |c| c.as_arr().iter().map(|c| file_size(out_root, c.get("file").and_then(Json::as_str))).sum())
        };
        let mut total: u64 = list.iter().map(size_of).sum();
        for s in list.iter_mut() {
            if total <= limit {
                break;
            }
            let before = size_of(s);
            prune_session(out_root, s, true, recycle, &mut freed, &|_| true);
            total -= before - size_of(s);
        }
    }
    list.retain(|s| {
        let has_match = !matches!(s.get("match"), Some(Json::Null) | None);
        let has_clips = s.get("clips").map_or(false, |c| !c.as_arr().is_empty());
        has_match || has_clips
    });
    let _ = save(out_root, &list);
    let _ = write_html(out_root, &list);
    if freed.files > 0 {
        log!("Limpieza: se borraron {}", freed.describe());
    }
    freed
}

/// Regenera la página quitando lo que el usuario haya borrado o movido.
fn write_html(out_root: &Path, list: &[Json]) -> std::io::Result<PathBuf> {
    let exists = |j: Option<&Json>| j.and_then(Json::as_str).map_or(false, |r| out_root.join(r).is_file());
    let mut visible = Vec::new();
    for s in list {
        let Json::Obj(fields) = s else { continue };
        let mut fields = fields.clone();
        for (k, v) in fields.iter_mut() {
            if k == "match" && !exists(Some(v)) {
                *v = Json::Null;
            }
            if k == "clips" {
                let kept: Vec<Json> = v.as_arr().iter().filter(|c| exists(c.get("file"))).cloned().collect();
                *v = Json::Arr(kept);
            }
        }
        let mut obj = Json::Obj(fields);
        // Tamaños, para mostrar en el reproductor cuánto ocupa cada cosa.
        let msize = file_size(out_root, obj.get("match").and_then(Json::as_str));
        obj.set("match_size", Json::Num(msize as f64));
        if let Some(Json::Arr(clips)) = obj.get_mut("clips") {
            for c in clips.iter_mut() {
                let sz = file_size(out_root, c.get("file").and_then(Json::as_str));
                c.set("size", Json::Num(sz as f64));
            }
        }
        let has_match = !matches!(obj.get("match"), Some(Json::Null) | None);
        let has_clips = obj.get("clips").map_or(false, |c| !c.as_arr().is_empty());
        if has_match || has_clips {
            visible.push(obj);
        }
    }
    let page = TEMPLATE.replacen(MARKER, &Json::Arr(visible).to_text(), 1);
    let path = html_path(out_root);
    std::fs::create_dir_all(out_root)?;
    std::fs::write(&path, page)?;
    Ok(path)
}

/// Dirección para abrir el reproductor: por el servidor local si está activo (permite silenciar
/// pistas), o directamente el archivo .html.
pub fn player_url(out_root: &Path, session: Option<&str>) -> String {
    let frag = session.map(|id| format!("#s={}", crate::util::url_encode(id))).unwrap_or_default();
    if let Some(base) = crate::server::base_url() {
        let _ = ensure_html(out_root);
        return format!("{base}{frag}");
    }
    let html = ensure_html(out_root);
    let p = html.to_string_lossy().replace('\\', "/");
    let p = p.trim_start_matches('/');
    format!("file:///{}{frag}", crate::util::url_encode(p))
}

/// Asegura que la página exista y esté al día (para abrirla desde el menú).
pub fn ensure_html(out_root: &Path) -> PathBuf {
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let list = load(out_root);
    write_html(out_root, &list).unwrap_or_else(|_| html_path(out_root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("cliplite_lib_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("G/Partidas")).unwrap();
        std::fs::create_dir_all(root.join("G/Clips")).unwrap();
        for (f, n) in [("G/Partidas/viejo.mp4", 1000), ("G/Clips/viejo_fav.mp4", 100), ("G/Clips/viejo_kill.mp4", 100), ("G/Partidas/hoy.mp4", 500), ("G/Clips/hoy_kill.mp4", 50)] {
            std::fs::write(root.join(f), vec![0u8; n]).unwrap();
        }
        let today = crate::util::timestamp();
        let lib = format!(
            r#"[{{"id":"viejo","game":"G","date":"2020-01-01_10-00-00","duration":60,"match":"G/Partidas/viejo.mp4","clips":[{{"file":"G/Clips/viejo_fav.mp4","label":"kill","t":1,"dur":5,"fav":true}},{{"file":"G/Clips/viejo_kill.mp4","label":"kill","t":9,"dur":5}}],"events":[]}},
               {{"id":"hoy","game":"G","date":"{today}","duration":30,"match":"G/Partidas/hoy.mp4","clips":[{{"file":"G/Clips/hoy_kill.mp4","label":"kill","t":2,"dur":5}}],"events":[]}}]"#
        );
        std::fs::create_dir_all(root.join(".cliplite")).unwrap();
        std::fs::write(data_path(&root), lib).unwrap();
        root
    }
    fn exists(root: &Path, f: &str) -> bool {
        root.join(f).is_file()
    }

    #[test]
    fn cleanup_by_age_keeps_favorites() {
        let root = setup("age");
        let f = cleanup(&root, 30, 0.0, false);
        assert_eq!((f.files, f.bytes), (2, 1100));
        assert!(!exists(&root, "G/Partidas/viejo.mp4") && !exists(&root, "G/Clips/viejo_kill.mp4"));
        assert!(exists(&root, "G/Clips/viejo_fav.mp4"), "el favorito debe conservarse");
        assert!(exists(&root, "G/Partidas/hoy.mp4") && exists(&root, "G/Clips/hoy_kill.mp4"));
        let list = load(&root);
        assert_eq!(list.len(), 2, "la sesión vieja sigue con su favorito");
        assert!(matches!(list[0].get("match"), Some(Json::Null)));
        assert_eq!(list[0].get("clips").unwrap().as_arr().len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cleanup_by_space_oldest_first() {
        let root = setup("space");
        // 1750 bytes en total; límite 700 bytes → se borra lo viejo (1100) y basta.
        let f = cleanup(&root, 0, 700e-9, false);
        assert_eq!(f.bytes, 1100);
        assert!(exists(&root, "G/Partidas/hoy.mp4"), "lo reciente se conserva si ya cabe");
        // Límite 120 bytes → también cae la partida de hoy; el favorito sigue.
        let f = cleanup(&root, 0, 120e-9, false);
        assert_eq!(f.bytes, 550);
        assert!(exists(&root, "G/Clips/viejo_fav.mp4"));
        assert_eq!(load(&root).len(), 1, "la sesión de hoy queda vacía y desaparece");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn manual_delete() {
        let root = setup("manual");
        set_fav(&root, "G/Clips/hoy_kill.mp4", true).unwrap();
        // Borrar la sesión vieja: se conserva su favorito; borrar a mano un favorito sí se permite.
        let f = delete_session(&root, "viejo", false);
        assert_eq!(f.files, 2);
        assert!(exists(&root, "G/Clips/viejo_fav.mp4"));
        let f = delete_file(&root, "G/Clips/viejo_fav.mp4", false);
        assert_eq!(f.files, 1);
        assert_eq!(load(&root).len(), 1);
        // Borrar la partida completa de hoy deja su clip.
        delete_file(&root, "G/Partidas/hoy.mp4", false);
        let list = load(&root);
        assert!(matches!(list[0].get("match"), Some(Json::Null)));
        assert!(exists(&root, "G/Clips/hoy_kill.mp4"));
        let _ = std::fs::remove_dir_all(root);
    }
}
