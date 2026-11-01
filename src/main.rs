// ClipLite — grabador de partidas y clips ultraligero.
//
// Diseño para impacto mínimo en el juego:
//  * La captura de pantalla (Desktop Duplication) y la codificación de vídeo se hacen en la GPU
//    (NVENC / AMF / Quick Sync) a través de FFmpeg; los fotogramas nunca pasan por la CPU.
//  * Este programa solo orquesta: no toca píxeles, duerme casi todo el tiempo y usa ~2-5 MB de RAM.
//  * Se graba en segmentos de 2 s; los clips se generan copiando segmentos (sin recodificar).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![allow(clippy::new_without_default)]

#[macro_use]
mod log;
mod audio;
mod config;
mod controller;
mod events;
mod json;
mod library;
mod platform;
mod recorder;
mod server;
mod util;

#[cfg(any(windows, feature = "checkwin"))]
#[allow(dead_code)]
mod win;

#[cfg(not(windows))]
mod headless;

fn main() {
    #[cfg(windows)]
    win::run();
    #[cfg(not(windows))]
    headless::run();
}
