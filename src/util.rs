// Utilidades independientes de la plataforma.
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Evita que se abra una ventana de consola al lanzar procesos (FFmpeg) en Windows.
pub fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Fecha y hora local con formato apto para nombres de archivo: 2026-11-01_14-33-05
pub fn timestamp() -> String {
    let (y, mo, d, h, mi, s) = crate::platform::local_time();
    format!("{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{s:02}")
}

/// Convierte segundos UNIX a fecha civil (UTC). Usado fuera de Windows.
#[allow(dead_code)]
pub fn civil_from_unix(secs: i64) -> (u32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (
        y as u32,
        m as u32,
        d as u32,
        (rem / 3600) as u32,
        (rem % 3600 / 60) as u32,
        (rem % 60) as u32,
    )
}

/// Días desde 1970-01-01 de una fecha civil (algoritmo de Howard Hinnant).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Días transcurridos desde una fecha "AAAA-MM-DD_hh-mm-ss" hasta hoy (hora local).
pub fn age_days(date: &str) -> Option<i64> {
    let mut it = date.split(|c| c == '-' || c == '_').map(|x| x.parse::<i64>());
    let (y, m, d) = (it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
    let (ny, nm, nd, ..) = crate::platform::local_time();
    Some(days_from_civil(ny as i64, nm as i64, nd as i64) - days_from_civil(y, m, d))
}

/// Tamaño legible: 1,4 GB / 830 MB.
pub fn human_size(b: u64) -> String {
    let gb = b as f64 / 1e9;
    if gb >= 1.0 {
        format!("{:.1} GB", gb).replace('.', ",")
    } else {
        format!("{:.0} MB", b as f64 / 1e6)
    }
}

/// Nombre legible del juego a partir del ejecutable.
pub fn friendly_game(exe: &str) -> String {
    let base = exe.trim_end_matches(".exe").trim_end_matches(".EXE");
    let lower = base.to_ascii_lowercase();
    let known = [
        ("league of legends", "League of Legends"),
        ("valorant", "VALORANT"),
        ("cs2", "Counter-Strike 2"),
        ("r5apex", "Apex Legends"),
        ("fortniteclient", "Fortnite"),
        ("overwatch", "Overwatch"),
        ("rocketleague", "Rocket League"),
        ("dota2", "Dota 2"),
        ("rainbowsix", "Rainbow Six Siege"),
        ("marvel-win64", "Marvel Rivals"),
        ("tslgame", "PUBG"),
        ("gta5", "GTA V"),
        ("cod", "Call of Duty"),
    ];
    for (k, v) in known {
        if lower.starts_with(k) {
            return v.to_string();
        }
    }
    sanitize(base.trim_end_matches("-Win64-Shipping"))
}

/// Elimina caracteres no válidos en nombres de archivo de Windows.
pub fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let out = out.trim().trim_end_matches('.').to_string();
    if out.is_empty() { "Juego".into() } else { out }
}

/// Devuelve una ruta que no exista todavía (añade _2, _3…).
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for i in 2..1000 {
        let p = dir.join(format!("{stem}_{i}.{ext}"));
        if !p.exists() {
            return p;
        }
    }
    first
}

/// Busca ffmpeg: ruta configurada, junto al ejecutable, o en el PATH.
pub fn find_ffmpeg(configured: &str) -> Option<PathBuf> {
    if !configured.trim().is_empty() {
        let p = PathBuf::from(configured.trim());
        if p.is_file() {
            return Some(p);
        }
    }
    let exe_name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for c in [
                dir.join(exe_name),
                dir.join("ffmpeg").join("bin").join(exe_name),
                dir.join("ffmpeg").join(exe_name),
            ] {
                if c.is_file() {
                    return Some(c);
                }
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            let c = d.join(exe_name);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

/// Codifica un texto para usarlo en una URL (deja tal cual letras, números y - . _ ~ / :).
pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Espera a que termine un proceso con límite de tiempo; si no, lo mata.
pub fn wait_timeout(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return Some(st),
            Ok(None) => {}
            Err(_) => return None,
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Segundos con signo entre dos instantes (b - a).
pub fn secs_between(a: Instant, b: Instant) -> f64 {
    if b >= a {
        (b - a).as_secs_f64()
    } else {
        -((a - b).as_secs_f64())
    }
}

/// Atajo de teclado tipo "Alt+F10" → (modificadores, código de tecla virtual de Windows).
pub fn parse_hotkey(s: &str) -> Option<(u32, u32)> {
    let mut mods = 0u32;
    let mut vk = None;
    for part in s.split('+') {
        let p = part.trim().to_ascii_uppercase();
        match p.as_str() {
            "" => {}
            "CTRL" | "CONTROL" => mods |= 0x2,
            "ALT" => mods |= 0x1,
            "SHIFT" | "MAYUS" | "MAYÚS" => mods |= 0x4,
            "WIN" => mods |= 0x8,
            k => vk = key_vk(k),
        }
    }
    vk.map(|v| (mods | 0x4000 /* MOD_NOREPEAT */, v))
}

fn key_vk(k: &str) -> Option<u32> {
    if let Some(n) = k.strip_prefix('F') {
        if let Ok(n) = n.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(0x70 + n - 1);
            }
        }
    }
    if let Some(n) = k.strip_prefix("NUMPAD") {
        if let Ok(n) = n.parse::<u32>() {
            if n <= 9 {
                return Some(0x60 + n);
            }
        }
    }
    if k.len() == 1 {
        let c = k.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    Some(match k {
        "INSERT" | "INS" => 0x2D,
        "DELETE" | "SUPR" | "DEL" => 0x2E,
        "HOME" | "INICIO" => 0x24,
        "END" | "FIN" => 0x23,
        "PAGEUP" | "REPAG" => 0x21,
        "PAGEDOWN" | "AVPAG" => 0x22,
        "PAUSE" | "PAUSA" => 0x13,
        "SCROLLLOCK" => 0x91,
        "PRINTSCREEN" | "IMPRPANT" => 0x2C,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hotkeys() {
        assert_eq!(parse_hotkey("Alt+F10"), Some((0x4001, 0x79)));
        assert_eq!(parse_hotkey("Ctrl+Shift+K"), Some((0x4006, b'K' as u32)));
        assert_eq!(parse_hotkey("F9"), Some((0x4000, 0x78)));
        assert_eq!(parse_hotkey("alt + numpad5"), Some((0x4001, 0x65)));
        assert_eq!(parse_hotkey("Alt+Patata"), None);
    }
    #[test]
    fn names() {
        assert_eq!(friendly_game("League of Legends.exe"), "League of Legends");
        assert_eq!(friendly_game("VALORANT-Win64-Shipping.exe"), "VALORANT");
        assert_eq!(friendly_game("MiJuego-Win64-Shipping.exe"), "MiJuego");
        assert_eq!(sanitize("a:b/c?"), "a_b_c_");
        assert_eq!(civil_from_unix(1_793_542_385), (2026, 11, 1, 14, 13, 5));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 11, 1), 1_793_542_385 / 86_400);
        assert_eq!(days_from_civil(2024, 3, 1) - days_from_civil(2024, 2, 28), 2);
        assert_eq!(human_size(1_400_000_000), "1,4 GB");
        assert_eq!(human_size(830_000_000), "830 MB");
    }
}
