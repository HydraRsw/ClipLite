// Detección automática de momentos importantes.
//  * League of Legends: API local oficial del cliente (https://127.0.0.1:2999), consultada 1 vez/s.
//  * Counter-Strike 2: Game State Integration oficial de Valve (el juego nos envía su estado).
// Ambas son APIs oficiales y de solo lectura: no tocan la memoria del juego (sin riesgo de anti-cheat).
use crate::controller::Cmd;
use crate::json::{self, Json};
use crate::platform::Fetcher;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn event(tx: &Sender<Cmd>, label: &str, prio: u8, at: Instant) {
    let _ = tx.send(Cmd::Event { label: label.to_string(), prio, at });
}

// ------------------------------------------------------------------ League of Legends

pub fn spawn_lol(mut fetch: Fetcher, wanted: Vec<String>, tx: Sender<Cmd>, stop: Arc<AtomicBool>) {
    let has = move |k: &str| wanted.iter().any(|w| w == k);
    std::thread::Builder::new()
        .name("lol".into())
        .spawn(move || {
            let mut me: HashSet<String> = HashSet::new();
            let mut last_id: i64 = -1;
            let mut first = true;
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1000));
                if me.is_empty() {
                    if let Some(j) = fetch("/liveclientdata/activeplayer").and_then(|s| json::parse(&s)) {
                        for k in ["riotId", "riotIdGameName", "summonerName"] {
                            if let Some(n) = j.get(k).and_then(Json::as_str) {
                                if !n.is_empty() {
                                    me.insert(n.to_lowercase());
                                    if let Some((a, _)) = n.split_once('#') {
                                        me.insert(a.to_lowercase());
                                    }
                                }
                            }
                        }
                    }
                    if me.is_empty() {
                        continue; // aún en pantalla de carga
                    }
                    log!("LoL: jugador detectado");
                }
                let Some(ev) = fetch("/liveclientdata/eventdata").and_then(|s| json::parse(&s)) else { continue };
                let polled = Instant::now();
                let game_time = fetch("/liveclientdata/gamestats")
                    .and_then(|s| json::parse(&s))
                    .and_then(|j| j.get("gameTime").and_then(Json::as_f64));
                let is_me = |j: &Json, k: &str| j.get(k).and_then(Json::as_str).map_or(false, |n| me.contains(&n.to_lowercase()));

                for e in ev.get("Events").map(Json::as_arr).unwrap_or(&[]) {
                    let id = e.get("EventID").and_then(Json::as_f64).unwrap_or(-1.0) as i64;
                    if id <= last_id {
                        continue;
                    }
                    last_id = id;
                    let et = e.get("EventTime").and_then(Json::as_f64).unwrap_or(0.0);
                    let ago = game_time.map(|g| (g - et).clamp(0.0, 10.0)).unwrap_or(0.5);
                    if first && ago >= 10.0 {
                        continue; // sucedió antes de empezar a grabar
                    }
                    let at = polled.checked_sub(Duration::from_secs_f64(ago)).unwrap_or(polled);
                    let name = e.get("EventName").and_then(Json::as_str).unwrap_or("");
                    match name {
                        "ChampionKill" => {
                            if is_me(e, "KillerName") && has("kills") {
                                event(&tx, "kill", 2, at);
                            } else if is_me(e, "VictimName") && has("muertes") {
                                event(&tx, "muerte", 1, at);
                            } else if has("asistencias")
                                && e.get("Assisters").map(Json::as_arr).unwrap_or(&[]).iter()
                                    .any(|a| a.as_str().map_or(false, |n| me.contains(&n.to_lowercase())))
                            {
                                event(&tx, "asistencia", 1, at);
                            }
                        }
                        "Multikill" if is_me(e, "KillerName") && has("multikills") => {
                            let n = e.get("KillStreak").and_then(Json::as_f64).unwrap_or(2.0) as u8;
                            let label = match n {
                                2 => "doublekill",
                                3 => "triplekill",
                                4 => "quadrakill",
                                _ => "pentakill",
                            };
                            event(&tx, label, 2 + n, at);
                        }
                        "Ace" if is_me(e, "Acer") && has("multikills") => event(&tx, "ace", 5, at),
                        "FirstBlood" if is_me(e, "Recipient") && has("kills") => event(&tx, "primera_sangre", 3, at),
                        "DragonKill" | "BaronKill" | "HeraldKill" | "HordeKill" | "AtakhanKill"
                            if is_me(e, "KillerName") && has("objetivos") =>
                        {
                            let label = match name {
                                "DragonKill" => "dragon",
                                "BaronKill" => "baron",
                                "HeraldKill" => "heraldo",
                                "AtakhanKill" => "atakhan",
                                _ => "larvas",
                            };
                            event(&tx, label, 2, at);
                        }
                        "TurretKilled" | "InhibKilled" if is_me(e, "KillerName") && has("objetivos") => {
                            event(&tx, if name == "TurretKilled" { "torre" } else { "inhibidor" }, 1, at)
                        }
                        _ => {}
                    }
                }
                first = false;
            }
        })
        .ok();
}

// ------------------------------------------------------------------ Counter-Strike 2 (GSI)

#[derive(Default)]
struct Cs2State {
    map: String,
    kills: Option<i64>,
    deaths: Option<i64>,
}

pub fn spawn_cs2(port: u16, wanted: Vec<String>, tx: Sender<Cmd>) {
    if wanted.is_empty() {
        return;
    }
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            log!("CS2: no se pudo abrir el puerto {port}: {e}");
            return;
        }
    };
    crate::platform::install_cs2_cfg(port);
    std::thread::Builder::new()
        .name("cs2".into())
        .spawn(move || {
            let mut st = Cs2State::default();
            for stream in listener.incoming().flatten() {
                if let Some(body) = read_http_body(stream) {
                    if let Some(j) = json::parse(&body) {
                        handle_cs2(&j, &mut st, &wanted, &tx);
                    }
                }
            }
        })
        .ok();
}

fn read_http_body(mut s: TcpStream) -> Option<String> {
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut buf = Vec::with_capacity(8192);
    let mut tmp = [0u8; 8192];
    let (head_end, len) = loop {
        let n = s.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..p]).to_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                .unwrap_or(0);
            break (p + 4, len);
        }
        if buf.len() > 64 * 1024 {
            return None;
        }
    };
    while buf.len() < head_end + len && buf.len() < 4 * 1024 * 1024 {
        let n = s.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let end = (head_end + len).min(buf.len());
    Some(String::from_utf8_lossy(&buf[head_end..end]).into_owned())
}

fn handle_cs2(j: &Json, st: &mut Cs2State, wanted: &[String], tx: &Sender<Cmd>) {
    let now = Instant::now();
    let map = j.path(&["map", "name"]).and_then(Json::as_str).unwrap_or("").to_string();
    if map != st.map {
        *st = Cs2State { map, ..Default::default() };
    }
    // Solo nuestros datos: si estamos muertos y espectando a otro, se ignora.
    let me = j.path(&["provider", "steamid"]).and_then(Json::as_str);
    let player = j.path(&["player", "steamid"]).and_then(Json::as_str);
    if me.is_none() || me != player {
        return;
    }
    let kills = j.path(&["player", "match_stats", "kills"]).and_then(Json::as_f64).map(|v| v as i64);
    let deaths = j.path(&["player", "match_stats", "deaths"]).and_then(Json::as_f64).map(|v| v as i64);
    let round_kills = j.path(&["player", "state", "round_kills"]).and_then(Json::as_f64).unwrap_or(0.0) as i64;

    if let (Some(prev), Some(k)) = (st.kills, kills) {
        if k > prev && wanted.iter().any(|w| w == "kills") {
            let (label, prio) = match round_kills {
                5.. => ("ace", 6),
                4 => ("4k", 5),
                3 => ("3k", 4),
                2 => ("2k", 3),
                _ => ("kill", 2),
            };
            event(tx, label, prio, now);
        }
    }
    if let (Some(prev), Some(d)) = (st.deaths, deaths) {
        if d > prev && wanted.iter().any(|w| w == "muertes") {
            event(tx, "muerte", 1, now);
        }
    }
    if kills.is_some() {
        st.kills = kills;
    }
    if deaths.is_some() {
        st.deaths = deaths;
    }
}

/// Contenido del archivo que CS2 necesita para enviarnos eventos.
pub fn cs2_cfg_text(port: u16) -> String {
    format!(
        "\"ClipLite\"\n{{\n  \"uri\" \"http://127.0.0.1:{port}\"\n  \"timeout\" \"1.0\"\n  \"buffer\" \"0.0\"\n  \"throttle\" \"0.1\"\n  \"heartbeat\" \"30.0\"\n  \"data\"\n  {{\n    \"provider\" \"1\"\n    \"map\" \"1\"\n    \"player_id\" \"1\"\n    \"player_state\" \"1\"\n    \"player_match_stats\" \"1\"\n  }}\n}}\n"
    )
}
