// Biblioteca de sesiones para el reproductor.
// Los datos viven en <carpeta>/.cliplite/biblioteca.json y se incrustan en <carpeta>/ClipLite.html,
// una página autónoma que se abre como ventana de aplicación al terminar cada partida.
use crate::json::{self, Json};
use crate::recorder::LibEntry;
use std::path::{Path, PathBuf};

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
    let dp = data_path(out_root);
    if let Some(d) = dp.parent() {
        std::fs::create_dir_all(d)?;
        crate::platform::hide_path(d);
    }
    std::fs::write(&dp, Json::Arr(list.clone()).to_text())?;
    let html = write_html(out_root, &list)?;
    Ok((html, id))
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
        let obj = Json::Obj(fields);
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

/// Asegura que la página exista y esté al día (para abrirla desde el menú).
pub fn ensure_html(out_root: &Path) -> PathBuf {
    let list = load(out_root);
    write_html(out_root, &list).unwrap_or_else(|_| html_path(out_root))
}
