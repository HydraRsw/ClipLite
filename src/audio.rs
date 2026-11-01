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

pub struct AudioThread {
    stop: Arc<AtomicBool>,
    stdin_tx: Option<SyncSender<ChildStdin>>,
    th: Option<JoinHandle<()>>,
}

impl AudioThread {
    /// Abre el dispositivo en su propio hilo y devuelve el formato para configurar FFmpeg.
    pub fn start() -> Result<(AudioThread, AudioFormat), String> {
        let (fmt_tx, fmt_rx) = sync_channel::<Result<AudioFormat, String>>(1);
        let (in_tx, in_rx) = sync_channel::<ChildStdin>(1);
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
                if let Ok(stdin) = in_rx.recv() {
                    pump(cap, stdin, fmt, &stop2);
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

    pub fn attach(&mut self, stdin: ChildStdin) {
        if let Some(tx) = self.stdin_tx.take() {
            let _ = tx.send(stdin);
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

fn pump(mut cap: Box<dyn Capture>, mut stdin: ChildStdin, fmt: AudioFormat, stop: &AtomicBool) {
    let bpf = fmt.bytes_per_frame().max(1) as u64;
    let bytes_per_sec = fmt.rate as u64 * bpf;
    let zeros = vec![0u8; (bytes_per_sec / 10) as usize]; // 100 ms
    let start = Instant::now();
    let mut written: u64 = 0;
    let mut buf: Vec<u8> = Vec::with_capacity(bytes_per_sec as usize / 5);
    let mut device_ok = true;
    let mut last_reopen = Instant::now();

    while !stop.load(Ordering::Relaxed) {
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

        if !buf.is_empty() {
            if stdin.write_all(&buf).is_err() {
                return;
            }
            written += buf.len() as u64;
        } else {
            let expected = start.elapsed().as_micros() as u64 * bytes_per_sec / 1_000_000 / bpf * bpf;
            let mut lag = expected.saturating_sub(written);
            if lag >= bytes_per_sec / 50 {
                while lag > 0 {
                    let n = lag.min(zeros.len() as u64) as usize;
                    if stdin.write_all(&zeros[..n]).is_err() {
                        return;
                    }
                    written += n as u64;
                    lag -= n as u64;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = stdin.flush();
}
