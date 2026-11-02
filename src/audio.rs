// Audio → FFmpeg por una sola tubería.
//
// Pista 1 ("Todo") = lo que suena por los altavoces; es la fuente principal y marca el reloj.
// Pistas extra = una por aplicación (juego, Discord, Spotify…) y el micrófono. Todas se entregan
// a 48 kHz estéreo float y se intercalan como canales de un único flujo: así comparten reloj y
// quedan sincronizadas entre sí. FFmpeg las separa luego en pistas de audio con nombre.
use std::collections::VecDeque;
use std::io::Write;
use std::process::ChildStdin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AudioFormat {
    pub rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub float: bool,
}

/// Formato común de todas las pistas cuando hay varias.
pub const MULTI: AudioFormat = AudioFormat { rate: 48_000, channels: 2, bits: 32, float: true };
const MULTI_BPF: usize = 8;

impl AudioFormat {
    pub fn ffmpeg_fmt(&self) -> &'static str {
        match (self.float, self.bits) {
            (true, 64) => "f64le",
            (true, _) => "f32le",
            (false, 16) => "s16le",
            (false, 24) => "s24le",
            (false, 32) => "s32le",
            _ => "u8",
        }
    }
    pub fn bytes_per_frame(&self) -> usize {
        self.channels as usize * (self.bits as usize / 8)
    }
}

/// Fuente de audio (WASAPI en Windows; generador de prueba en otros sistemas).
pub trait Capture {
    fn format(&self) -> AudioFormat;
    /// Añade a `out` todo el audio disponible ahora mismo, sin bloquear.
    fn read(&mut self, out: &mut Vec<u8>) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq)]
pub enum TrackSrc {
    /// Todo lo que suena por la salida predeterminada.
    System,
    /// Solo una aplicación (y sus procesos hijos).
    Process(u32),
    /// Micrófono predeterminado.
    Mic,
}

#[derive(Clone, Debug)]
pub struct TrackSpec {
    pub name: String,
    pub src: TrackSrc,
}

/// Lo que FFmpeg necesita saber del flujo de audio.
#[derive(Clone, Debug)]
pub struct OutInfo {
    pub fmt: AudioFormat,
    /// Nombre de cada pista (la 0 es la mezcla completa).
    pub tracks: Vec<TrackSpec>,
}

impl OutInfo {
    pub fn mic_track(&self) -> Option<usize> {
        self.tracks.iter().position(|t| t.src == TrackSrc::Mic)
    }
}

static STALL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub type Clock = Arc<std::sync::Mutex<Option<Instant>>>;

pub struct AudioThread {
    stop: Arc<AtomicBool>,
    stdin_tx: Option<SyncSender<(ChildStdin, Clock)>>,
    th: Option<JoinHandle<()>>,
}

impl AudioThread {
    /// Abre las fuentes en su propio hilo y devuelve el formato para configurar FFmpeg.
    /// `strict`: mantener todas las pistas pedidas aunque alguna no se pueda abrir (se graba
    /// en silencio), para que todas las partes de una partida tengan las mismas pistas.
    pub fn start(specs: Vec<TrackSpec>, strict: bool) -> Result<(AudioThread, OutInfo), String> {
        let (fmt_tx, fmt_rx) = sync_channel::<Result<OutInfo, String>>(1);
        let (in_tx, in_rx) = sync_channel::<(ChildStdin, Clock)>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let th = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let want_multi = specs.len() > 1;
                let master = match crate::platform::open_source(&TrackSrc::System, want_multi) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = fmt_tx.send(Err(e));
                        return;
                    }
                };
                let mut tracks = vec![specs[0].clone()];
                let mut secs = Vec::new();
                if want_multi && master.format() == MULTI {
                    for spec in &specs[1..] {
                        let cap = match crate::platform::open_source(&spec.src, true) {
                            Ok(c) if c.format() == MULTI => Some(c),
                            Ok(_) => {
                                log!("Pista '{}': formato no compatible", spec.name);
                                None
                            }
                            Err(e) => {
                                log!("Pista '{}' no disponible: {e}", spec.name);
                                None
                            }
                        };
                        if cap.is_some() || strict {
                            tracks.push(spec.clone());
                            secs.push(Sec { cap, fifo: VecDeque::new(), primed: false, tmp: Vec::new(), name: spec.name.clone() });
                        }
                    }
                } else if want_multi {
                    log!("Pistas separadas no disponibles: la salida de audio no admite 48 kHz estéreo");
                }
                let fmt = if secs.is_empty() {
                    master.format()
                } else {
                    AudioFormat { channels: 2 * (1 + secs.len() as u16), ..MULTI }
                };
                let _ = fmt_tx.send(Ok(OutInfo { fmt, tracks }));
                if let Ok((stdin, clock)) = in_rx.recv() {
                    pump(master, secs, stdin, &stop2, clock);
                }
            })
            .map_err(|e| e.to_string())?;
        match fmt_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(info)) => Ok((AudioThread { stop, stdin_tx: Some(in_tx), th: Some(th) }, info)),
            Ok(Err(e)) => {
                let _ = th.join();
                Err(e)
            }
            Err(_) => Err("el dispositivo de audio no respondió".into()),
        }
    }

    /// `clock` = instante real del fotograma 0 del vídeo (lo mide la grabadora).
    pub fn attach(&mut self, stdin: ChildStdin, clock: Clock) {
        if let Some(tx) = self.stdin_tx.take() {
            let _ = tx.send((stdin, clock));
        }
    }

    /// Cierra la tubería: FFmpeg ve el fin del audio y termina limpiamente (-shortest).
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.stdin_tx = None;
        if let Some(th) = self.th.take() {
            let _ = th.join();
        }
    }
}

/// Fuente secundaria con un pequeño colchón (40 ms) que absorbe el retraso variable de sus paquetes.
struct Sec {
    cap: Option<Box<dyn Capture>>,
    fifo: VecDeque<u8>,
    primed: bool,
    tmp: Vec<u8>,
    name: String,
}

const SEC_PREBUF: usize = 48_000 * MULTI_BPF * 40 / 1000;
const SEC_MAX: usize = 48_000 * MULTI_BPF * 300 / 1000;

impl Sec {
    fn poll(&mut self) {
        let Some(cap) = self.cap.as_mut() else { return };
        self.tmp.clear();
        if let Err(e) = cap.read(&mut self.tmp) {
            log!("Pista '{}': se perdió la fuente ({e}); seguirá en silencio", self.name);
            self.cap = None;
            return;
        }
        self.fifo.extend(self.tmp.iter());
        if self.fifo.len() > SEC_MAX {
            let extra = (self.fifo.len() - SEC_PREBUF) / MULTI_BPF * MULTI_BPF;
            self.fifo.drain(..extra);
        }
        if self.fifo.len() >= SEC_PREBUF {
            self.primed = true;
        }
    }
    fn reset(&mut self) {
        self.fifo.clear();
        self.primed = false;
    }
}

/// Entrega el audio a FFmpeg desde un hilo propio. Así la captura NUNCA espera a FFmpeg:
/// si FFmpeg deja de leer un rato (pasa al cargar una partida), el audio se guarda en memoria
/// y se entrega después, sin perder nada y con sus tiempos intactos.
struct Writer {
    tx: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    queued: Arc<std::sync::atomic::AtomicU64>,
    dead: Arc<AtomicBool>,
    max_queued: u64,
}

/// Límite de seguridad de la cola (si FFmpeg se quedara colgado de verdad).
const WRITER_LIMIT: u64 = 512 * 1024 * 1024;

impl Writer {
    fn new(mut stdin: ChildStdin) -> Writer {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let queued = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let dead = Arc::new(AtomicBool::new(false));
        let (q2, d2) = (queued.clone(), dead.clone());
        let spawned = std::thread::Builder::new().name("audio-ffmpeg".into()).spawn(move || {
            while let Ok(buf) = rx.recv() {
                // Solo pruebas: simula que FFmpeg deja de leer un rato.
                let n = STALL.swap(u64::MAX, Ordering::SeqCst);
                if n > 0 && n != u64::MAX {
                    std::thread::sleep(Duration::from_secs(n));
                } else if n == 0 {
                    STALL.store(0, Ordering::SeqCst);
                }
                if stdin.write_all(&buf).is_err() {
                    d2.store(true, Ordering::SeqCst);
                    break;
                }
                q2.fetch_sub(buf.len() as u64, Ordering::SeqCst);
            }
            let _ = stdin.flush();
            // Al soltar `stdin`, FFmpeg ve el fin del audio y termina (-shortest).
        });
        if spawned.is_err() {
            dead.store(true, Ordering::SeqCst);
        }
        Writer { tx: Some(tx), queued, dead, max_queued: 0 }
    }

    fn send(&mut self, data: Vec<u8>) -> bool {
        if self.dead.load(Ordering::SeqCst) {
            return false;
        }
        let q = self.queued.fetch_add(data.len() as u64, Ordering::SeqCst) + data.len() as u64;
        self.max_queued = self.max_queued.max(q);
        if q > WRITER_LIMIT {
            log!("Audio: FFmpeg lleva demasiado tiempo sin leer el audio; se reinicia la captura");
            return false;
        }
        self.tx.as_ref().map_or(false, |tx| tx.send(data).is_ok())
    }
}

/// Envía `master` (N fotogramas de la pista principal) intercalando N fotogramas de cada pista extra.
fn emit(w: &mut Writer, master: &[u8], bpf: usize, secs: &mut [Sec], written: &mut u64) -> bool {
    if master.is_empty() {
        return true;
    }
    *written += master.len() as u64;
    if secs.is_empty() {
        return w.send(master.to_vec());
    }
    let frames = master.len() / bpf;
    let mut out = Vec::with_capacity(frames * (bpf + MULTI_BPF * secs.len()));
    for f in 0..frames {
        out.extend_from_slice(&master[f * bpf..(f + 1) * bpf]);
        for s in secs.iter_mut() {
            if s.primed && s.fifo.len() >= MULTI_BPF {
                out.extend(s.fifo.drain(..MULTI_BPF));
            } else {
                s.primed = false; // se vació: volver a llenar el colchón
                out.extend_from_slice(&[0u8; MULTI_BPF]);
            }
        }
    }
    w.send(out)
}

fn pump(mut master: Box<dyn Capture>, mut secs: Vec<Sec>, stdin: ChildStdin, stop: &AtomicBool, clock: Clock) {
    // Este hilo apenas usa CPU, pero no debe quedarse sin turno aunque el juego use el 100 %.
    crate::platform::boost_audio_thread();
    let mut writer = Writer::new(stdin);
    // Solo pruebas: CLIPLITE_STALL="a,n" simula que FFmpeg deja de leer el audio n segundos
    // a partir del segundo a de grabación.
    let stall: Option<(u64, u64)> = std::env::var("CLIPLITE_STALL").ok().and_then(|v| {
        let (a, n) = v.split_once(',')?;
        Some((a.parse().ok()?, n.parse().ok()?))
    });
    let fmt = master.format();
    let bpf = fmt.bytes_per_frame().max(1);
    let bpf64 = bpf as u64;
    let bytes_per_sec = fmt.rate as u64 * bpf64;
    let align = |b: u64| b / bpf64 * bpf64;
    let ms = |m: u64| align(bytes_per_sec * m / 1000);
    let zeros = vec![0u8; ms(100) as usize];
    let ms_of = |b: f64| b * 1000.0 / bytes_per_sec as f64;

    // Objetivo: la muestra n del audio corresponde al instante (fotograma 0 del vídeo + n/frecuencia).
    //  1. Al conocer el fotograma 0 se alinea UNA vez (se descarta o se rellena lo que sobre/falte).
    //  2. Después se escribe el audio tal cual llega. Los paquetes de Windows llegan con retraso
    //     variable: eso se tolera, nunca se recorta ni se rellena por ello.
    //  3. Si el sistema deja de entregar audio (nada suena), se rellena con silencio.
    //  4. La pequeña desviación del reloj de la tarjeta se corrige quitando o duplicando una sola
    //     muestra cada 10 ms (inaudible) cuando el desfase medio supera 40 ms.
    let start = Instant::now();
    let mut written: u64 = 0;
    let mut buf: Vec<u8> = Vec::with_capacity(bytes_per_sec as usize / 5);
    let _ = master.read(&mut buf); // descartar lo capturado antes de arrancar
    for s in secs.iter_mut() {
        s.poll();
        s.reset();
    }
    let mut device_ok = true;
    let mut last_reopen = Instant::now();
    let mut last_data = Instant::now();
    let mut aligned = false;
    let mut skip: u64 = 0;
    let mut err_avg: f64 = 0.0; // desfase medio (bytes): + adelantado, - atrasado
    let mut nudging = false;
    let mut nudges: u64 = 0;
    let mut fills: u64 = 0;

    'main: while !stop.load(Ordering::Relaxed) {
        buf.clear();
        if device_ok {
            if let Err(e) = master.read(&mut buf) {
                log!("Audio: se perdió el dispositivo ({e}); se grabará silencio mientras tanto");
                device_ok = false;
                last_reopen = Instant::now();
            }
        } else if last_reopen.elapsed() > Duration::from_secs(2) {
            // Ej. se cambió de altavoces a auriculares: reintentar con el nuevo dispositivo.
            last_reopen = Instant::now();
            if let Ok(c) = crate::platform::open_source(&TrackSrc::System, fmt == MULTI) {
                if c.format() == fmt {
                    master = c;
                    device_ok = true;
                    log!("Audio: dispositivo recuperado");
                }
            }
        }
        for s in secs.iter_mut() {
            s.poll();
        }

        let video0 = clock.lock().ok().and_then(|g| *g);
        let reference = video0.unwrap_or(start);
        let expected = align(Instant::now().saturating_duration_since(reference).as_micros() as u64 * bytes_per_sec / 1_000_000);

        // 1. Alineación inicial con el vídeo (una sola vez).
        if !aligned && video0.is_some() {
            aligned = true;
            for s in secs.iter_mut() {
                s.reset();
            }
            if written > expected {
                skip = written - expected;
            } else {
                let mut left = expected - written;
                while left > 0 {
                    let n = left.min(zeros.len() as u64) as usize;
                    if !emit(&mut writer, &zeros[..n], bpf, &mut secs, &mut written) {
                        break 'main;
                    }
                    left -= n as u64;
                }
            }
            log!("Audio: alineado con el vídeo ({:+.0} ms)", -ms_of(skip as f64));
        }

        if !buf.is_empty() {
            last_data = Instant::now();
            let mut data: Vec<u8> = std::mem::take(&mut buf);
            if skip > 0 {
                let k = skip.min(data.len() as u64) as usize;
                data.drain(..k);
                skip -= k as u64;
            }
            // 4. Corrección de deriva, muy gradual.
            if aligned && data.len() >= 4 * bpf {
                let err = written as f64 + data.len() as f64 - expected as f64;
                err_avg += (err - err_avg) * 0.02;
                let e = ms_of(err_avg);
                if e.abs() > 40.0 {
                    nudging = true;
                } else if e.abs() < 10.0 {
                    nudging = false;
                }
                if nudging {
                    nudges += 1;
                    if e > 0.0 {
                        data.truncate(data.len() - bpf); // adelantado: quitar una muestra
                    } else {
                        let last = data[data.len() - bpf..].to_vec();
                        data.extend_from_slice(&last); // atrasado: repetir una muestra
                    }
                }
            }
            if !emit(&mut writer, &data, bpf, &mut secs, &mut written) {
                break 'main;
            }
            buf = data;
            // Protección ante un desfase enorme (p. ej. el sistema se quedó congelado).
            if aligned && written > expected + ms(500) {
                skip = written - expected;
            }
        } else if last_data.elapsed() > Duration::from_millis(100) {
            // 3. No suena nada por los altavoces: rellenar con silencio hasta el instante actual
            //    (las pistas extra, p. ej. el micrófono, siguen avanzando con sus datos).
            let lag = expected.saturating_sub(written);
            if lag >= ms(20) {
                fills += 1;
                let mut left = lag;
                while left > 0 {
                    let n = left.min(zeros.len() as u64) as usize;
                    if !emit(&mut writer, &zeros[..n], bpf, &mut secs, &mut written) {
                        break 'main;
                    }
                    left -= n as u64;
                }
                err_avg = 0.0;
            }
        }
        if let Some((a, n)) = stall {
            if start.elapsed() >= Duration::from_secs(a) && STALL.load(Ordering::SeqCst) == 0 {
                STALL.store(n, Ordering::SeqCst);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out_bps = fmt.rate as f64 * (bpf + MULTI_BPF * secs.len()) as f64;
    log!(
        "Audio: fin. Desfase medio {:+.0} ms, {} microajustes de reloj, {} rellenos de silencio, máximo esperando a FFmpeg {:.1} s",
        ms_of(err_avg),
        nudges,
        fills,
        writer.max_queued as f64 / out_bps
    );
    drop(writer); // el hilo escritor termina de entregar lo pendiente y cierra la tubería
}
