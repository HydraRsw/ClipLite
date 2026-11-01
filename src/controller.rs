// Controlador: un solo hilo que recibe órdenes (atajos, menú, juegos detectados, eventos)
// y gestiona la sesión de grabación. Duerme bloqueado en un canal; despierta 4 veces/s solo
// mientras graba, para revisar segmentos.
use crate::config::{Config, Mode};
use crate::recorder::{self, Encoder, Job, Session};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum Cmd {
    GameStarted(String),
    GameStopped(String),
    ToggleRecord,
    SaveClip,
    Event { label: String, prio: u8, at: Instant },
    SetMode(Mode),
    SetFolder(PathBuf),
    Reload,
    OpenLibrary,
    Notice(String),
    Quit,
}

#[derive(Clone)]
pub enum UiMsg {
    State { recording: bool, game: String, mode: Mode, folder: PathBuf, encoder: String },
    Notice(String),
    Exit,
}

pub type Notify = Arc<dyn Fn(UiMsg) + Send + Sync>;
pub type FetcherFactory = Arc<dyn Fn() -> Option<crate::platform::Fetcher> + Send + Sync>;

#[derive(PartialEq)]
enum Source {
    Auto(String),
    Manual,
}

pub struct Controller {
    cfg: Config,
    cfg_path: PathBuf,
    ffmpeg: PathBuf,
    tx: Sender<Cmd>,
    notify: Notify,
    jobs: Sender<Job>,
    worker: Option<std::thread::JoinHandle<()>>,
    encoder: Option<Encoder>,
    session: Option<(Session, Source)>,
    running_game: Option<String>,
    lol_stop: Option<Arc<AtomicBool>>,
    lol_factory: FetcherFactory,
    restarts: u32,
}

impl Controller {
    pub fn new(
        cfg: Config,
        cfg_path: PathBuf,
        ffmpeg: PathBuf,
        tx: Sender<Cmd>,
        notify: Notify,
        lol_factory: FetcherFactory,
    ) -> Controller {
        let (jobs, jobs_rx) = channel();
        let worker = recorder::spawn_worker(ffmpeg.clone(), jobs_rx, tx.clone());
        Controller {
            cfg,
            cfg_path,
            ffmpeg,
            tx,
            notify,
            jobs,
            worker: Some(worker),
            encoder: None,
            session: None,
            running_game: None,
            lol_stop: None,
            lol_factory,
            restarts: 0,
        }
    }

    pub fn run(mut self, rx: Receiver<Cmd>) {
        let _ = std::fs::create_dir_all(&self.cfg.output_dir);
        crate::server::start(self.cfg.output_dir.clone(), self.ffmpeg.clone());
        recorder::recover_leftovers(&self.cfg.output_dir.join(".buffer"), &self.cfg.output_dir, &self.jobs);
        self.encoder = recorder::probe_encoder(&self.ffmpeg, &self.cfg);
        match &self.encoder {
            Some(e) => log!("Codificador elegido: {}", e.label),
            None => self.notice("No se encontró ningún codificador que funcione. Revisa el registro (log.txt)."),
        }
        crate::events::spawn_cs2(self.cfg.cs2_port, self.cfg.cs2_events.clone(), self.tx.clone());
        self.push_state();

        loop {
            let timeout = if self.session.is_some() { Duration::from_millis(250) } else { Duration::from_secs(3600) };
            match rx.recv_timeout(timeout) {
                Ok(Cmd::Quit) => break,
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.tick();
        }

        self.stop_session(false);
        // Esperar a que terminen de guardarse clips/partidas antes de salir.
        let (dead_tx, _) = channel();
        self.jobs = dead_tx;
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        (self.notify)(UiMsg::Exit);
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::GameStarted(exe) => {
                self.running_game = Some(exe.clone());
                self.restarts = 0;
                if self.cfg.autodetect && self.session.is_none() {
                    self.start_session(&exe, Source::Auto(exe.clone()));
                }
            }
            Cmd::GameStopped(exe) => {
                self.running_game = None;
                if matches!(&self.session, Some((_, Source::Auto(g))) if *g == exe) {
                    self.stop_session(self.cfg.open_player);
                }
            }
            Cmd::ToggleRecord => {
                if self.session.is_some() {
                    self.stop_session(self.cfg.open_player);
                } else {
                    let game = self.running_game.clone().unwrap_or_else(|| "Escritorio".into());
                    self.start_session(&game, Source::Manual);
                }
            }
            Cmd::SaveClip => {
                let (secs, now) = (self.cfg.manual_secs as f64, Instant::now());
                match self.session.as_mut() {
                    Some((s, _)) => {
                        s.request_clip(now, secs, 1.0, "manual", 10, true);
                        self.beep();
                    }
                    None => self.notice("No se está grabando: no hay nada que guardar"),
                }
            }
            Cmd::Event { label, prio, at } => {
                let (pre, post) = (self.cfg.pre as f64, self.cfg.post as f64);
                if let Some((s, _)) = self.session.as_mut() {
                    log!("Momento detectado: {label}");
                    s.request_clip(at, pre, post, &label, prio, false);
                }
            }
            Cmd::SetMode(m) => {
                self.cfg.mode = m;
                self.save_cfg();
                if let Some((s, _)) = self.session.as_mut() {
                    s.set_mode(m);
                }
                self.notice(match m {
                    Mode::Completa => "Modo: grabar la partida completa",
                    Mode::Momentos => "Modo: solo momentos importantes",
                });
            }
            Cmd::SetFolder(p) => {
                self.cfg.output_dir = p;
                self.save_cfg();
                crate::server::set_root(self.cfg.output_dir.clone());
                let _ = std::fs::create_dir_all(&self.cfg.output_dir);
                let extra = if self.session.is_some() { " (se aplicará en la próxima grabación)" } else { "" };
                self.notice(&format!("Carpeta: {}{extra}", self.cfg.output_dir.display()));
            }
            Cmd::Reload => {
                let new = Config::load_or_create(&self.cfg_path);
                let enc_changed = new.encoder != self.cfg.encoder
                    || new.codec != self.cfg.codec
                    || new.bitrate_mbps != self.cfg.bitrate_mbps
                    || new.monitor != self.cfg.monitor;
                self.cfg = new;
                crate::server::set_root(self.cfg.output_dir.clone());
                if enc_changed {
                    self.encoder = recorder::probe_encoder(&self.ffmpeg, &self.cfg);
                }
                self.notice("Configuración recargada");
            }
            Cmd::OpenLibrary => {
                crate::platform::open_url(&crate::library::player_url(&self.cfg.output_dir, None));
            }
            Cmd::Notice(msg) => self.notice(&msg),
            Cmd::Quit => {}
        }
        self.push_state();
    }

    fn tick(&mut self) {
        let alive = match self.session.as_mut() {
            Some((s, _)) => s.tick(&self.jobs),
            None => return,
        };
        if !alive {
            // FFmpeg se cayó (p. ej. cambio de resolución o pantalla UAC): guardar y reanudar.
            let (s, src) = self.session.take().unwrap();
            let game = match &src {
                Source::Auto(g) => g.clone(),
                Source::Manual => s.game().to_string(),
            };
            s.stop(&self.jobs, false);
            self.stop_lol();
            self.restarts += 1;
            if self.restarts <= 5 {
                std::thread::sleep(Duration::from_secs(1));
                log!("Reanudando grabación (intento {})", self.restarts);
                self.start_session(&game, src);
            } else {
                self.notice("La grabación falló varias veces. Revisa log.txt");
            }
            self.push_state();
        }
    }

    fn start_session(&mut self, game: &str, src: Source) {
        if self.encoder.is_none() {
            self.encoder = recorder::probe_encoder(&self.ffmpeg, &self.cfg);
        }
        let Some(enc) = self.encoder.clone() else {
            self.notice("No hay codificador de vídeo disponible");
            return;
        };
        match Session::start(&self.cfg, &self.ffmpeg, &enc, game) {
            Ok(s) => {
                if game.eq_ignore_ascii_case("League of Legends.exe") && !self.cfg.lol_events.is_empty() {
                    if let Some(f) = (self.lol_factory)() {
                        let stop = Arc::new(AtomicBool::new(false));
                        crate::events::spawn_lol(f, self.cfg.lol_events.clone(), self.tx.clone(), stop.clone());
                        self.lol_stop = Some(stop);
                    }
                }
                self.session = Some((s, src));
                self.beep();
            }
            Err(e) => {
                log!("{e}");
                self.notice(&e);
            }
        }
    }

    fn stop_lol(&mut self) {
        if let Some(s) = self.lol_stop.take() {
            s.store(true, Ordering::Relaxed);
        }
    }

    fn stop_session(&mut self, open_player: bool) {
        self.stop_lol();
        if let Some((s, _)) = self.session.take() {
            s.stop(&self.jobs, open_player);
            self.beep();
        }
    }

    fn save_cfg(&self) {
        if let Err(e) = self.cfg.save(&self.cfg_path) {
            log!("No se pudo guardar la configuración: {e}");
        }
    }

    fn beep(&self) {
        if self.cfg.sound {
            (self.notify)(UiMsg::Notice(String::new()));
        }
    }

    fn notice(&self, msg: &str) {
        log!("{msg}");
        if self.cfg.notifications || msg.is_empty() {
            (self.notify)(UiMsg::Notice(msg.to_string()));
        }
    }

    fn push_state(&self) {
        (self.notify)(UiMsg::State {
            recording: self.session.is_some(),
            game: self.session.as_ref().map(|(s, _)| s.game().to_string()).unwrap_or_default(),
            mode: self.cfg.mode,
            folder: self.cfg.output_dir.clone(),
            encoder: self.encoder.as_ref().map(|e| e.label.clone()).unwrap_or_else(|| "ninguno".into()),
        });
    }
}
