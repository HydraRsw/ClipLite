// Banco de pruebas fuera de Windows: ejecuta el mismo controlador y motor de grabación con
// una fuente de vídeo/audio sintética y simula juegos, eventos de LoL/CS2 y atajos.
use crate::config::{Config, Mode};
use crate::controller::{Cmd, Controller, Notify, UiMsg};
use std::io::Write;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Duration;

pub fn run() {
    let mode = if std::env::args().any(|a| a == "completa") { Mode::Completa } else { Mode::Momentos };
    let out = std::path::PathBuf::from(std::env::var("CLIPLITE_OUT").unwrap_or_else(|_| "cliplite_out".into()));
    let _ = std::fs::create_dir_all(crate::platform::config_dir());
    crate::log::init(&crate::platform::config_dir().join("log.txt"));
    let cfg_path = crate::config::config_path();
    let mut cfg = Config::load_or_create(&cfg_path);
    cfg.output_dir = out;
    cfg.mode = mode;
    cfg.fps = 30;
    cfg.bitrate_mbps = 4;
    cfg.pre = 3;
    cfg.post = 2;
    cfg.manual_secs = 6;
    cfg.cs2_port = 3999;
    cfg.cs2_events = vec!["kills".into(), "muertes".into()];
    let ffmpeg = crate::util::find_ffmpeg("").expect("ffmpeg en el PATH");

    let (tx, rx) = channel();
    let notify: Notify = Arc::new(|m| match m {
        UiMsg::State { recording, game, mode, encoder, .. } => {
            println!("[UI] grabando={recording} juego='{game}' modo={} enc={encoder}", mode.as_str())
        }
        UiMsg::Notice(s) if s.is_empty() => println!("[UI] *beep*"),
        UiMsg::Notice(s) => println!("[UI] aviso: {s}"),
        UiMsg::Exit => println!("[UI] salir"),
    });
    // LoL simulado: tras 2 s aparece una kill nuestra y luego una doble kill.
    let t0 = std::time::Instant::now();
    let lol: crate::controller::FetcherFactory = Arc::new(move || {
        Some(Box::new(move |path: &str| {
            let gt = t0.elapsed().as_secs_f64();
            Some(match path {
                "/liveclientdata/activeplayer" => r#"{"riotId":"Nelson#LAN","riotIdGameName":"Nelson"}"#.into(),
                "/liveclientdata/gamestats" => format!(r#"{{"gameTime":{gt}}}"#),
                _ => {
                    let mut ev = vec![r#"{"EventID":0,"EventName":"GameStart","EventTime":0.0}"#.to_string()];
                    if gt > 6.0 {
                        ev.push(r#"{"EventID":1,"EventName":"ChampionKill","EventTime":5.5,"KillerName":"Nelson","VictimName":"X","Assisters":[]}"#.into());
                    }
                    if gt > 8.0 {
                        ev.push(r#"{"EventID":2,"EventName":"ChampionKill","EventTime":7.6,"KillerName":"Nelson","VictimName":"Y","Assisters":[]}"#.into());
                        ev.push(r#"{"EventID":3,"EventName":"Multikill","EventTime":7.6,"KillerName":"Nelson","KillStreak":2}"#.into());
                    }
                    format!(r#"{{"Events":[{}]}}"#, ev.join(","))
                }
            })
        }) as crate::platform::Fetcher)
    });

    let ctrl = Controller::new(cfg, cfg_path, ffmpeg, tx.clone(), notify, lol);
    let th = std::thread::spawn(move || ctrl.run(rx));
    let sleep = |s: f64| std::thread::sleep(Duration::from_secs_f64(s));

    sleep(1.0);
    println!("--- se abre League of Legends");
    tx.send(Cmd::GameStarted("League of Legends.exe".into())).unwrap();
    sleep(14.0);
    println!("--- atajo de clip manual");
    tx.send(Cmd::SaveClip).unwrap();
    sleep(2.0);
    println!("--- CS2 envía estado (0 kills) y luego 1 kill");
    for k in [0, 1] {
        let body = format!(
            r#"{{"provider":{{"steamid":"765"}},"map":{{"name":"de_dust2"}},"player":{{"steamid":"765","state":{{"round_kills":{k}}},"match_stats":{{"kills":{k},"deaths":0}}}}}}"#
        );
        if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", 3999)) {
            let _ = write!(s, "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
            let mut r = String::new();
            let _ = std::io::Read::read_to_string(&mut s, &mut r);
        }
        sleep(0.5);
    }
    sleep(4.0);
    println!("--- se cierra el juego");
    tx.send(Cmd::GameStopped("League of Legends.exe".into())).unwrap();
    sleep(1.0);
    tx.send(Cmd::Quit).unwrap();
    let _ = th.join();
}
