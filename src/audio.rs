// Audio del sistema → FFmpeg por una tubería.
// Si no suena nada, el dispositivo no entrega datos; aquí rellenamos con silencio para que el
// flujo sea continuo (si no, los segmentos y la sincronía se rompen).
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

/// Fuente de audio (WASAPI loopback en Windows; generador de prueba en otros sistemas).
pub trait Capture {
    fn format(&self) -> AudioFormat;
    /// Añade a `out` todo el audio disponible ahora mismo, sin bloquear.
    fn read(&mut self, out: &mut Vec<u8>) -> Result<(), String>;
}

pub type Clock = Arc<std::sync::Mutex<Option<Instant>>>;

pub struct AudioThread {
    stop: Arc<AtomicBool>,
    stdin_tx: Option<SyncSender<(ChildStdin, Clock)>>,
    th: Option<JoinHandle<()>>,
}

impl AudioThread {
    /// Abre el dispositivo en su propio hilo y devuelve el formato para configurar FFmpeg.
    pub fn start() -> Result<(AudioThread, AudioFormat), String> {
        let (fmt_tx, fmt_rx) = sync_channel::<Result<AudioFormat, String>>(1);
        let (in_tx, in_rx) = sync_channel::<(ChildStdin, Clock)>(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let th = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || {
                let cap = match crate::platform::open_audio() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = fmt_tx.send(Err(e));
                        return;
                    }
                };
                let fmt = cap.format();
                let _ = fmt_tx.send(Ok(fmt));
                if let Ok((stdin, clock)) = in_rx.recv() {
                    pump(cap, stdin, fmt, &stop2, clock);
                }
            })
            .map_err(|e| e.to_string())?;
        match fmt_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(fmt)) => Ok((AudioThread { stop, stdin_tx: Some(in_tx), th: Some(th) }, fmt)),
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

fn pump(mut cap: Box<dyn Capture>, mut stdin: ChildStdin, fmt: AudioFormat, stop: &AtomicBool, clock: Clock) {
    let bpf = fmt.bytes_per_frame().max(1) as u64;
    let bytes_per_sec = fmt.rate as u64 * bpf;
    let align = |b: u64| b / bpf * bpf;
    let ms = |m: u64| align(bytes_per_sec * m / 1000);
    let zeros = vec![0u8; ms(100) as usize];
    let ms_of = |b: f64| b * 1000.0 / bytes_per_sec as f64;

    // Objetivo: la muestra n del audio corresponde al instante (fotograma 0 del vídeo + n/frecuencia).
    //  1. Al conocer el fotograma 0 se alinea UNA vez (se descarta o se rellena lo que sobre/falte).
    //  2. Después se escribe el audio tal cual llega. Los paquetes de Windows llegan con retraso
    //     variable: eso se tolera, nunca se recorta ni se rellena por ello (eso causaba cortes).
    //  3. Si el sistema deja de entregar audio (nada suena), se rellena con silencio.
    //  4. La pequeña desviación del reloj de la tarjeta se corrige quitando o duplicando una sola
    //     muestra cada 10 ms (inaudible) cuando el desfase medio supera 40 ms.
    let start = Instant::now();
    let mut written: u64 = 0;
    let mut buf: Vec<u8> = Vec::with_capacity(bytes_per_sec as usize / 5);
    let _ = cap.read(&mut buf); // descartar lo capturado antes de arrancar
    let mut device_ok = true;
    let mut last_reopen = Instant::now();
    let mut last_data = Instant::now();
    let mut aligned = false;
    let mut skip: u64 = 0;
    let mut err_avg: f64 = 0.0; // desfase medio (bytes): + adelantado, - atrasado
    let mut nudging = false;
    let mut nudges: u64 = 0;
    let mut fills: u64 = 0;

    let write = |stdin: &mut ChildStdin, data: &[u8], written: &mut u64| -> bool {
        if data.is_empty() {
            return true;
        }
        *written += data.len() as u64;
        stdin.write_all(data).is_ok()
    };

    'main: while !stop.load(Ordering::Relaxed) {
        buf.clear();
        if device_ok {
            if let Err(e) = cap.read(&mut buf) {
                log!("Audio: se perdió el dispositivo ({e}); se grabará silencio mientras tanto");
                device_ok = false;
                last_reopen = Instant::now();
            }
        } else if last_reopen.elapsed() > Duration::from_secs(2) {
            // Ej. se cambió de altavoces a auriculares: reintentar con el nuevo dispositivo.
            last_reopen = Instant::now();
            if let Ok(c) = crate::platform::open_audio() {
                if c.format() == fmt {
                    cap = c;
                    device_ok = true;
                    log!("Audio: dispositivo recuperado");
                }
            }
        }

        let video0 = clock.lock().ok().and_then(|g| *g);
        let reference = video0.unwrap_or(start);
        let expected = align(Instant::now().saturating_duration_since(reference).as_micros() as u64 * bytes_per_sec / 1_000_000);

        // 1. Alineación inicial con el vídeo (una sola vez).
        if !aligned && video0.is_some() {
            aligned = true;
            if written > expected {
                skip = written - expected;
            } else {
                let mut left = expected - written;
                while left > 0 {
                    let n = left.min(zeros.len() as u64) as usize;
                    if !write(&mut stdin, &zeros[..n], &mut written) {
                        break 'main;
                    }
                    left -= n as u64;
                }
            }
            log!("Audio: alineado con el vídeo ({:+.0} ms)", -ms_of(skip as f64));
        }

        if !buf.is_empty() {
            last_data = Instant::now();
            let mut data: &[u8] = &buf;
            if skip > 0 {
                let k = skip.min(data.len() as u64) as usize;
                data = &data[k..];
                skip -= k as u64;
            }
            // 4. Corrección de deriva, muy gradual.
            if aligned && data.len() as u64 >= 4 * bpf {
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
                    let b = bpf as usize;
                    if e > 0.0 {
                        data = &data[..data.len() - b]; // adelantado: quitar una muestra
                    } else {
                        let last = data[data.len() - b..].to_vec();
                        if !write(&mut stdin, data, &mut written) || !write(&mut stdin, &last, &mut written) {
                            break 'main;
                        }
                        data = &[];
                    }
                }
            }
            if !write(&mut stdin, data, &mut written) {
                break 'main;
            }
            // Protección ante un desfase enorme (p. ej. el sistema se quedó congelado).
            if aligned && written > expected + ms(500) {
                skip = written - expected;
            }
        } else if last_data.elapsed() > Duration::from_millis(100) {
            // 3. No suena nada: rellenar con silencio hasta el instante actual.
            let lag = expected.saturating_sub(written);
            if lag >= ms(20) {
                fills += 1;
                let mut left = lag;
                while left > 0 {
                    let n = left.min(zeros.len() as u64) as usize;
                    if !write(&mut stdin, &zeros[..n], &mut written) {
                        break 'main;
                    }
                    left -= n as u64;
                }
                err_avg = 0.0;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = stdin.flush();
    log!(
        "Audio: fin. Desfase medio {:+.0} ms, {} microajustes de reloj, {} rellenos de silencio",
        ms_of(err_avg),
        nudges,
        fills
    );
}
