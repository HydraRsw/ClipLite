// Actualizaciones automáticas desde las "Releases" de GitHub.
//
// 1. Cada compilación de GitHub Actions publica una versión (v1.0.N) con ClipLite.exe.
// 2. La app consulta la última versión (al rato de abrirse y cada 6 h). Si es más nueva, descarga
//    el .exe en segundo plano y comprueba su huella SHA-256 (la que publica GitHub): un archivo
//    incompleto o alterado nunca se instala.
// 3. Avisa ("Hay una actualización nueva") y se instala al pulsar "Actualizar ahora" o, si no,
//    sola la próxima vez que se abre ClipLite. Nunca durante una grabación.
use crate::json::{self, Json};
use crate::util::VERSION;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const REPO: &str = "HydraRsw/ClipLite";
const ASSET: &str = "ClipLite.exe";

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub sha256: Option<String>,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct Ready {
    pub version: String,
    pub notes: String,
    pub file: PathBuf,
}

/// Actualización descargada y lista para instalar (la consultan la bandeja y el reproductor).
static READY: Mutex<Option<Ready>> = Mutex::new(None);

pub fn ready() -> Option<Ready> {
    READY.lock().unwrap().clone()
}

/// "v1.0.15" / "1.0.15-local" → (1, 0, 15). Una compilación local cuenta como versión 0.
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    if s.contains("local") {
        return Some((0, 0, 0));
    }
    let mut it = s.split('.').map(|x| x.parse::<u32>());
    Some((it.next()?.ok()?, it.next().unwrap_or(Ok(0)).ok()?, it.next().unwrap_or(Ok(0)).ok()?))
}

pub fn is_newer(remote: &str, local: &str) -> bool {
    match (parse_version(remote), parse_version(local)) {
        (Some(r), Some(l)) => r > l,
        _ => false,
    }
}

/// Lee la respuesta de la API de GitHub para la última versión publicada.
pub fn parse_release(text: &str) -> Result<Release, String> {
    let j = json::parse(text).ok_or("respuesta no válida")?;
    let version = j.get("tag_name").and_then(Json::as_str).ok_or("sin versión")?.trim_start_matches('v').to_string();
    let notes = j.get("body").and_then(Json::as_str).unwrap_or("").trim().to_string();
    let asset = j
        .get("assets")
        .map(Json::as_arr)
        .unwrap_or(&[])
        .iter()
        .find(|a| a.get("name").and_then(Json::as_str) == Some(ASSET))
        .ok_or("la versión no incluye ClipLite.exe")?;
    let url = asset.get("browser_download_url").and_then(Json::as_str).ok_or("sin enlace de descarga")?.to_string();
    if !url.starts_with("https://github.com/") {
        return Err("enlace de descarga inesperado".into());
    }
    let sha256 = asset.get("digest").and_then(Json::as_str).and_then(|d| d.strip_prefix("sha256:")).map(|d| d.to_lowercase());
    let size = asset.get("size").and_then(Json::as_f64).unwrap_or(0.0) as u64;
    Ok(Release { version, notes, url, sha256, size })
}

pub fn update_dir() -> PathBuf {
    crate::platform::config_dir().join("actualizacion")
}

/// Comprueba si hay versión nueva y, si la hay, la descarga y verifica.
/// Devuelve Ok(None) si ya está al día.
pub fn check_and_download() -> Result<Option<Ready>, String> {
    let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = crate::platform::https_get(&api)?;
    let rel = parse_release(&String::from_utf8_lossy(&body))?;
    if !is_newer(&rel.version, VERSION) {
        return Ok(None);
    }
    if let Some(r) = ready() {
        if r.version == rel.version && r.file.is_file() {
            return Ok(Some(r));
        }
    }
    let dir = update_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let part = dir.join("descarga.part");
    crate::platform::https_download(&rel.url, &part)?;
    let data = std::fs::read(&part).map_err(|e| e.to_string())?;
    let verdict = verify(&data, &rel);
    if let Err(e) = verdict {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    let file = dir.join(format!("ClipLite-{}.exe", rel.version));
    std::fs::rename(&part, &file).map_err(|e| e.to_string())?;
    // Borrar descargas anteriores.
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path() != file {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let r = Ready { version: rel.version, notes: rel.notes, file };
    *READY.lock().unwrap() = Some(r.clone());
    log!("Actualización {} descargada y verificada", r.version);
    Ok(Some(r))
}

fn verify(data: &[u8], rel: &Release) -> Result<(), String> {
    if data.len() < 64 * 1024 || &data[..2] != b"MZ" {
        return Err("la descarga no es un programa válido".into());
    }
    if rel.size > 0 && data.len() as u64 != rel.size {
        return Err("la descarga está incompleta".into());
    }
    match &rel.sha256 {
        Some(expected) => {
            let got = hex(&sha256(data));
            if &got != expected {
                return Err("la huella SHA-256 no coincide; se descarta la descarga".into());
            }
            Ok(())
        }
        None => Err("GitHub no publicó la huella del archivo; no se instala por seguridad".into()),
    }
}

/// Busca una actualización descargada anteriormente (por ejemplo, antes de reiniciar el PC).
pub fn load_pending() -> Option<Ready> {
    let dir = update_dir();
    let mut best: Option<Ready> = None;
    for e in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(v) = name.strip_prefix("ClipLite-").and_then(|n| n.strip_suffix(".exe")) else { continue };
        if is_newer(v, VERSION) && best.as_ref().map_or(true, |b| is_newer(v, &b.version)) {
            best = Some(Ready { version: v.to_string(), notes: String::new(), file: e.path() });
        }
    }
    if best.is_some() {
        *READY.lock().unwrap() = best.clone();
    }
    best
}

/// Quita la actualización ya instalada (o inválida).
pub fn clear() {
    *READY.lock().unwrap() = None;
    let _ = std::fs::remove_dir_all(update_dir());
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

// ------------------------------------------------------------------ SHA-256 (FIPS 180-4)

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
    0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
    0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
    0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
    0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    let mut out = [0u8; 32];
    for i in 0..8 {
        out[4 * i..4 * i + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_vectors() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(hex(&sha256(long)), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        let million = vec![b'a'; 1_000_000];
        assert_eq!(hex(&sha256(&million)), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn versions() {
        assert!(is_newer("v1.0.15", "1.0.9"));
        assert!(!is_newer("1.0.9", "1.0.15"));
        assert!(!is_newer("1.0.15", "1.0.15"));
        assert!(is_newer("1.0.1", "1.0.0-local"));
        assert!(is_newer("2.0.0", "1.9.99"));
        assert!(!is_newer("basura", "1.0.0"));
    }

    #[test]
    fn release_json() {
        let txt = r#"{"tag_name":"v1.0.20","body":"Novedades","assets":[
            {"name":"ClipLite-Windows.zip","browser_download_url":"https://github.com/x/zip","size":5},
            {"name":"ClipLite.exe","browser_download_url":"https://github.com/HydraRsw/ClipLite/releases/download/v1.0.20/ClipLite.exe","size":900000,"digest":"sha256:ABCDEF"}]}"#;
        let r = parse_release(txt).unwrap();
        assert_eq!((r.version.as_str(), r.size, r.sha256.as_deref()), ("1.0.20", 900000, Some("abcdef")));
        assert!(parse_release(r#"{"tag_name":"v1","assets":[{"name":"ClipLite.exe","browser_download_url":"https://evil.example/x.exe"}]}"#).is_err());
        assert!(parse_release(r#"{"tag_name":"v1","assets":[]}"#).is_err());
    }

    #[test]
    fn verify_rules() {
        let mut exe = vec![0u8; 100_000];
        exe[0] = b'M';
        exe[1] = b'Z';
        let good = Release { version: "9".into(), notes: String::new(), url: String::new(), sha256: Some(hex(&sha256(&exe))), size: exe.len() as u64 };
        assert!(verify(&exe, &good).is_ok());
        let mut bad = exe.clone();
        bad[500] = 1;
        assert!(verify(&bad, &good).is_err(), "alterado");
        assert!(verify(&exe[..90_000], &good).is_err(), "incompleto");
        assert!(verify(&exe, &Release { sha256: None, ..good.clone() }).is_err(), "sin huella");
        assert!(verify(b"hola", &good).is_err());
    }
}
