// Todo lo específico de Windows, llamando directamente a la API del sistema (sin librerías).
//  * Icono en la bandeja + menú, atajos globales, notificaciones.
//  * Audio del sistema con WASAPI en modo loopback (lo mismo que suena por los altavoces).
//  * Detección de juegos (lista de procesos cada 3 s).
//  * Cliente HTTPS local para la API de League of Legends (WinHTTP).
#![allow(non_snake_case, non_upper_case_globals, clippy::missing_transmute_annotations)]

use crate::audio::{AudioFormat, Capture};
use crate::config::{Config, Mode};
use crate::controller::{Cmd, Controller, FetcherFactory, Notify, UiMsg};
use crate::platform::Fetcher;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::mem::{size_of, transmute, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

type H = isize;
type BOOL = i32;
type HRESULT = i32;
type WndProc = unsafe extern "system" fn(H, u32, usize, isize) -> isize;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GUID(u32, u16, u16, [u8; 8]);

#[repr(C)]
struct POINT {
    x: i32,
    y: i32,
}
#[repr(C)]
struct MSG {
    hwnd: H,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: POINT,
}
#[repr(C)]
struct WNDCLASSEXW {
    cb_size: u32,
    style: u32,
    wnd_proc: WndProc,
    cls_extra: i32,
    wnd_extra: i32,
    instance: H,
    icon: H,
    cursor: H,
    background: H,
    menu_name: *const u16,
    class_name: *const u16,
    icon_sm: H,
}
#[repr(C)]
struct NOTIFYICONDATAW {
    cb_size: u32,
    hwnd: H,
    uid: u32,
    flags: u32,
    callback_msg: u32,
    icon: H,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    timeout_or_version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: GUID,
    balloon_icon: H,
}
#[repr(C)]
struct PROCESSENTRY32W {
    size: u32,
    usage: u32,
    pid: u32,
    heap_id: usize,
    module_id: u32,
    threads: u32,
    parent_pid: u32,
    pri_class_base: i32,
    flags: u32,
    exe_file: [u16; 260],
}
#[repr(C)]
#[derive(Default)]
struct SYSTEMTIME {
    year: u16,
    month: u16,
    dow: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    ms: u16,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassExW(wc: *const WNDCLASSEXW) -> u16;
    fn CreateWindowExW(ex: u32, class: *const u16, name: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: H, menu: H, inst: H, param: *mut c_void) -> H;
    fn DefWindowProcW(h: H, m: u32, w: usize, l: isize) -> isize;
    fn GetMessageW(msg: *mut MSG, h: H, min: u32, max: u32) -> BOOL;
    fn TranslateMessage(msg: *const MSG) -> BOOL;
    fn DispatchMessageW(msg: *const MSG) -> isize;
    fn PostMessageW(h: H, m: u32, w: usize, l: isize) -> BOOL;
    fn PostQuitMessage(code: i32);
    fn DestroyWindow(h: H) -> BOOL;
    fn RegisterHotKey(h: H, id: i32, mods: u32, vk: u32) -> BOOL;
    fn UnregisterHotKey(h: H, id: i32) -> BOOL;
    fn CreatePopupMenu() -> H;
    fn AppendMenuW(m: H, flags: u32, id: usize, text: *const u16) -> BOOL;
    fn TrackPopupMenu(m: H, flags: u32, x: i32, y: i32, reserved: i32, h: H, rect: *const c_void) -> BOOL;
    fn DestroyMenu(m: H) -> BOOL;
    fn GetCursorPos(p: *mut POINT) -> BOOL;
    fn SetForegroundWindow(h: H) -> BOOL;
    fn CreateIcon(inst: H, w: i32, h: i32, planes: u8, bpp: u8, and: *const u8, xor: *const u8) -> H;
    fn MessageBoxW(h: H, text: *const u16, caption: *const u16, t: u32) -> i32;
    fn RegisterWindowMessageW(s: *const u16) -> u32;
    fn MessageBeep(t: u32) -> BOOL;
}
#[link(name = "shell32")]
extern "system" {
    fn Shell_NotifyIconW(msg: u32, data: *mut NOTIFYICONDATAW) -> BOOL;
    fn ShellExecuteW(h: H, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> H;
}
#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> H;
    fn CreateMutexW(attr: *const c_void, owner: BOOL, name: *const u16) -> H;
    fn GetLastError() -> u32;
    fn SetPriorityClass(p: H, class: u32) -> BOOL;
    fn GetCurrentProcess() -> H;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> H;
    fn Process32FirstW(s: H, e: *mut PROCESSENTRY32W) -> BOOL;
    fn Process32NextW(s: H, e: *mut PROCESSENTRY32W) -> BOOL;
    fn CloseHandle(h: H) -> BOOL;
    fn GetLocalTime(t: *mut SYSTEMTIME);
    fn SetFileAttributesW(p: *const u16, a: u32) -> BOOL;
    fn GetFileAttributesW(p: *const u16) -> u32;
}
#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(r: *const c_void, flags: u32) -> HRESULT;
    fn CoCreateInstance(clsid: *const GUID, outer: *mut c_void, ctx: u32, iid: *const GUID, out: *mut *mut c_void) -> HRESULT;
    fn CoTaskMemFree(p: *mut c_void);
}
#[link(name = "advapi32")]
extern "system" {
    fn RegGetValueW(key: H, sub: *const u16, val: *const u16, flags: u32, ty: *mut u32, data: *mut c_void, cb: *mut u32) -> i32;
    fn RegSetKeyValueW(key: H, sub: *const u16, val: *const u16, ty: u32, data: *const c_void, cb: u32) -> i32;
    fn RegDeleteKeyValueW(key: H, sub: *const u16, val: *const u16) -> i32;
}
#[link(name = "winhttp")]
extern "system" {
    fn WinHttpOpen(agent: *const u16, access: u32, proxy: *const u16, bypass: *const u16, flags: u32) -> H;
    fn WinHttpSetTimeouts(h: H, resolve: i32, connect: i32, send: i32, recv: i32) -> BOOL;
    fn WinHttpConnect(h: H, server: *const u16, port: u16, reserved: u32) -> H;
    fn WinHttpOpenRequest(h: H, verb: *const u16, obj: *const u16, ver: *const u16, referrer: *const u16, accept: *const *const u16, flags: u32) -> H;
    fn WinHttpSetOption(h: H, opt: u32, buf: *const c_void, len: u32) -> BOOL;
    fn WinHttpSendRequest(h: H, headers: *const u16, hlen: u32, opt: *const c_void, olen: u32, total: u32, ctx: usize) -> BOOL;
    fn WinHttpReceiveResponse(h: H, reserved: *mut c_void) -> BOOL;
    fn WinHttpQueryDataAvailable(h: H, avail: *mut u32) -> BOOL;
    fn WinHttpReadData(h: H, buf: *mut c_void, len: u32, read: *mut u32) -> BOOL;
    fn WinHttpCloseHandle(h: H) -> BOOL;
}

// HKEY_CURRENT_USER es un LONG con signo extendido a puntero.
const HKCU: H = 0x8000_0001u32 as i32 as isize;
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

const WM_DESTROY: u32 = 0x0002;
const WM_HOTKEY: u32 = 0x0312;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_TRAY: u32 = 0x8001;
const WM_UPDATE: u32 = 0x8002;

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;

const MF_GRAYED: u32 = 0x1;
const MF_CHECKED: u32 = 0x8;
const MF_SEPARATOR: u32 = 0x800;

const ID_TOGGLE: usize = 10;
const ID_CLIP: usize = 11;
const ID_MODE_FULL: usize = 20;
const ID_MODE_MOM: usize = 21;
const ID_FOLDER: usize = 30;
const ID_OPEN: usize = 31;
const ID_EDIT: usize = 32;
const ID_RELOAD: usize = 33;
const ID_LOG: usize = 34;
const ID_LIBRARY: usize = 35;
const ID_AUTOSTART: usize = 40;
const ID_QUIT: usize = 99;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
fn wp(p: &Path) -> Vec<u16> {
    w(&p.to_string_lossy())
}
unsafe fn from_wide_ptr(p: *const u16) -> String {
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}
fn copy_wide(dst: &mut [u16], s: &str) {
    let v: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..v.len()].copy_from_slice(&v);
    dst[v.len()] = 0;
}

// ------------------------------------------------------------------ COM mínimo

unsafe fn vt(obj: *mut c_void, idx: usize) -> usize {
    *(*(obj as *const *const usize)).add(idx)
}
unsafe fn release(obj: *mut c_void) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = transmute(vt(obj, 2));
        f(obj);
    }
}
fn hr(code: HRESULT, what: &str) -> Result<(), String> {
    if code < 0 {
        Err(format!("{what} (error 0x{:08X})", code as u32))
    } else {
        Ok(())
    }
}

// ------------------------------------------------------------------ Funciones de plataforma

pub fn local_time() -> (u32, u32, u32, u32, u32, u32) {
    let mut t = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut t) };
    (t.year as u32, t.month as u32, t.day as u32, t.hour as u32, t.minute as u32, t.second as u32)
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("ClipLite")
}

pub fn default_output_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|p| PathBuf::from(p).join("Videos"))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ClipLite")
}

pub fn hide_path(p: &Path) {
    unsafe {
        let wpath = wp(p);
        let a = GetFileAttributesW(wpath.as_ptr());
        if a != u32::MAX && a & 0x2 == 0 {
            SetFileAttributesW(wpath.as_ptr(), a | 0x2);
        }
    }
}

fn reg_get_str(key: H, sub: &str, val: &str) -> Option<String> {
    let mut buf = [0u16; 2048];
    let mut cb = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(key, w(sub).as_ptr(), w(val).as_ptr(), 0x2 /* RRF_RT_REG_SZ */, null_mut(), buf.as_mut_ptr() as *mut c_void, &mut cb)
    };
    if r != 0 {
        return None;
    }
    let n = (cb as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// Instala el archivo de Game State Integration en todas las bibliotecas de Steam con CS2.
pub fn install_cs2_cfg(port: u16) {
    let Some(steam) = reg_get_str(HKCU, r"Software\Valve\Steam", "SteamPath") else { return };
    let steam = PathBuf::from(steam.replace('/', "\\"));
    let mut libs = vec![steam.clone()];
    if let Ok(vdf) = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")) {
        for line in vdf.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("\"path\"") {
                if let Some(v) = rest.trim().strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
                    libs.push(PathBuf::from(v.replace("\\\\", "\\")));
                }
            }
        }
    }
    let text = crate::events::cs2_cfg_text(port);
    for lib in libs {
        let dir = lib.join(r"steamapps\common\Counter-Strike Global Offensive\game\csgo\cfg");
        if dir.is_dir() {
            let f = dir.join("gamestate_integration_cliplite.cfg");
            if std::fs::read_to_string(&f).ok().as_deref() != Some(text.as_str()) && std::fs::write(&f, &text).is_ok() {
                log!("CS2: integración instalada en {} (reinicia CS2 si estaba abierto)", f.display());
            }
        }
    }
}

fn url_encode(s: &str) -> String {
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

/// Abre el reproductor como ventana de aplicación (Edge en modo app, incluido en Windows);
/// si no está disponible, con el navegador predeterminado.
pub fn open_player(html: &Path, session: Option<&str>) {
    let mut url = format!("file:///{}", url_encode(&html.to_string_lossy().replace('\\', "/")));
    if let Some(id) = session {
        url.push_str("#s=");
        url.push_str(&url_encode(id));
    }
    unsafe {
        let args = w(&format!("--app=\"{url}\" --window-size=1440,920"));
        let r = ShellExecuteW(0, w("open").as_ptr(), w("msedge.exe").as_ptr(), args.as_ptr(), null(), 1);
        if r <= 32 {
            ShellExecuteW(0, w("open").as_ptr(), wp(html).as_ptr(), null(), null(), 1);
        }
    }
}

fn autostart_enabled() -> bool {
    reg_get_str(HKCU, RUN_KEY, "ClipLite").is_some()
}

fn set_autostart(on: bool) {
    unsafe {
        if on {
            let exe = std::env::current_exe().unwrap_or_default();
            let val = w(&format!("\"{}\"", exe.display()));
            RegSetKeyValueW(HKCU, w(RUN_KEY).as_ptr(), w("ClipLite").as_ptr(), 1, val.as_ptr() as *const c_void, (val.len() * 2) as u32);
        } else {
            RegDeleteKeyValueW(HKCU, w(RUN_KEY).as_ptr(), w("ClipLite").as_ptr());
        }
    }
}

fn running_processes() -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(0x2 /* TH32CS_SNAPPROCESS */, 0);
        if snap == -1 {
            return out;
        }
        let mut e: PROCESSENTRY32W = zeroed();
        e.size = size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let n = e.exe_file.iter().position(|&c| c == 0).unwrap_or(260);
                out.push(String::from_utf16_lossy(&e.exe_file[..n]).to_lowercase());
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

// ------------------------------------------------------------------ WinHTTP (API local de LoL)

struct Http {
    session: H,
    conn: H,
}
impl Drop for Http {
    fn drop(&mut self) {
        unsafe {
            WinHttpCloseHandle(self.conn);
            WinHttpCloseHandle(self.session);
        }
    }
}

pub fn lol_fetcher() -> Option<Fetcher> {
    unsafe {
        let session = WinHttpOpen(w("ClipLite").as_ptr(), 1 /* sin proxy */, null(), null(), 0);
        if session == 0 {
            return None;
        }
        WinHttpSetTimeouts(session, 500, 500, 500, 1500);
        let conn = WinHttpConnect(session, w("127.0.0.1").as_ptr(), 2999, 0);
        if conn == 0 {
            WinHttpCloseHandle(session);
            return None;
        }
        let http = Http { session, conn };
        Some(Box::new(move |path: &str| http_get(&http, path)))
    }
}

fn http_get(h: &Http, path: &str) -> Option<String> {
    unsafe {
        let req = WinHttpOpenRequest(h.conn, w("GET").as_ptr(), w(path).as_ptr(), null(), null(), null(), 0x0080_0000 /* SECURE */);
        if req == 0 {
            return None;
        }
        // El certificado del cliente de LoL es autofirmado y solo escucha en 127.0.0.1.
        let flags: u32 = 0x100 | 0x200 | 0x1000 | 0x2000;
        WinHttpSetOption(req, 31 /* SECURITY_FLAGS */, &flags as *const u32 as *const c_void, 4);
        let mut out: Vec<u8> = Vec::new();
        let ok = WinHttpSendRequest(req, null(), 0, null(), 0, 0, 0) != 0 && WinHttpReceiveResponse(req, null_mut()) != 0;
        if ok {
            loop {
                let mut avail = 0u32;
                if WinHttpQueryDataAvailable(req, &mut avail) == 0 || avail == 0 || out.len() > 8 << 20 {
                    break;
                }
                let start = out.len();
                out.resize(start + avail as usize, 0);
                let mut read = 0u32;
                if WinHttpReadData(req, out[start..].as_mut_ptr() as *mut c_void, avail, &mut read) == 0 {
                    out.truncate(start);
                    break;
                }
                out.truncate(start + read as usize);
            }
        }
        WinHttpCloseHandle(req);
        ok.then(|| String::from_utf8_lossy(&out).into_owned())
    }
}

// ------------------------------------------------------------------ WASAPI loopback

const CLSID_MMDeviceEnumerator: GUID = GUID(0xBCDE0395, 0xE52F, 0x467C, [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E]);
const IID_IMMDeviceEnumerator: GUID = GUID(0xA95664D2, 0x9614, 0x4F35, [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6]);
const IID_IAudioClient: GUID = GUID(0x1CB9AD4C, 0xDBFA, 0x4C32, [0xB1, 0x78, 0xC2, 0xF5, 0x68, 0xA7, 0x03, 0xB2]);
const IID_IAudioCaptureClient: GUID = GUID(0xC8ADBD64, 0xE71E, 0x48A0, [0xA4, 0xDE, 0x18, 0x5C, 0x39, 0x5C, 0xD3, 0x17]);

struct Wasapi {
    client: *mut c_void,
    capture: *mut c_void,
    fmt: AudioFormat,
}

impl Drop for Wasapi {
    fn drop(&mut self) {
        unsafe {
            let stop: unsafe extern "system" fn(*mut c_void) -> HRESULT = transmute(vt(self.client, 11));
            stop(self.client);
            release(self.capture);
            release(self.client);
        }
    }
}

pub fn open_audio() -> Result<Box<dyn Capture>, String> {
    unsafe {
        CoInitializeEx(null(), 0 /* COINIT_MULTITHREADED */);
        let mut en = null_mut();
        hr(CoCreateInstance(&CLSID_MMDeviceEnumerator, null_mut(), 0x17, &IID_IMMDeviceEnumerator, &mut en), "MMDeviceEnumerator")?;
        let get_default: unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> HRESULT = transmute(vt(en, 4));
        let mut dev = null_mut();
        let r = get_default(en, 0 /* eRender */, 0 /* eConsole */, &mut dev);
        release(en);
        hr(r, "no hay dispositivo de salida de audio")?;

        let activate: unsafe extern "system" fn(*mut c_void, *const GUID, u32, *mut c_void, *mut *mut c_void) -> HRESULT = transmute(vt(dev, 3));
        let mut client = null_mut();
        let r = activate(dev, &IID_IAudioClient, 0x17, null_mut(), &mut client);
        release(dev);
        hr(r, "IAudioClient")?;

        let get_mix: unsafe extern "system" fn(*mut c_void, *mut *mut u8) -> HRESULT = transmute(vt(client, 8));
        let mut wf: *mut u8 = null_mut();
        if let Err(e) = hr(get_mix(client, &mut wf), "GetMixFormat") {
            release(client);
            return Err(e);
        }
        let rd16 = |o: usize| std::ptr::read_unaligned(wf.add(o) as *const u16);
        let rd32 = |o: usize| std::ptr::read_unaligned(wf.add(o) as *const u32);
        let tag = rd16(0);
        let float = tag == 3 || (tag == 0xFFFE && rd16(16) >= 22 && rd32(24) == 3);
        let fmt = AudioFormat { channels: rd16(2), rate: rd32(4), bits: rd16(14), float };

        let init: unsafe extern "system" fn(*mut c_void, i32, u32, i64, i64, *const u8, *const GUID) -> HRESULT = transmute(vt(client, 3));
        // Modo compartido + LOOPBACK, búfer de 1 s (no cuesta nada y tolera pausas del hilo).
        let r = init(client, 0, 0x0002_0000, 10_000_000, 0, wf, null());
        CoTaskMemFree(wf as *mut c_void);
        if let Err(e) = hr(r, "Initialize(loopback)") {
            release(client);
            return Err(e);
        }
        let get_service: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT = transmute(vt(client, 14));
        let mut capture = null_mut();
        if let Err(e) = hr(get_service(client, &IID_IAudioCaptureClient, &mut capture), "IAudioCaptureClient") {
            release(client);
            return Err(e);
        }
        let start: unsafe extern "system" fn(*mut c_void) -> HRESULT = transmute(vt(client, 10));
        let w = Wasapi { client, capture, fmt };
        hr(start(client), "Start")?;
        log!("Audio: {} Hz, {} canales, {} bits{}", fmt.rate, fmt.channels, fmt.bits, if float { " float" } else { "" });
        Ok(Box::new(w))
    }
}

impl Capture for Wasapi {
    fn format(&self) -> AudioFormat {
        self.fmt
    }
    fn read(&mut self, out: &mut Vec<u8>) -> Result<(), String> {
        unsafe {
            let next: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT = transmute(vt(self.capture, 5));
            let get: unsafe extern "system" fn(*mut c_void, *mut *mut u8, *mut u32, *mut u32, *mut u64, *mut u64) -> HRESULT = transmute(vt(self.capture, 3));
            let rel: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT = transmute(vt(self.capture, 4));
            let bpf = self.fmt.bytes_per_frame();
            loop {
                let mut n = 0u32;
                hr(next(self.capture, &mut n), "GetNextPacketSize")?;
                if n == 0 {
                    return Ok(());
                }
                let (mut data, mut frames, mut flags) = (null_mut(), 0u32, 0u32);
                hr(get(self.capture, &mut data, &mut frames, &mut flags, null_mut(), null_mut()), "GetBuffer")?;
                let bytes = frames as usize * bpf;
                if flags & 0x2 != 0 || data.is_null() {
                    out.resize(out.len() + bytes, 0); // AUDCLNT_BUFFERFLAGS_SILENT
                } else {
                    out.extend_from_slice(std::slice::from_raw_parts(data, bytes));
                }
                rel(self.capture, frames);
            }
        }
    }
}

// ------------------------------------------------------------------ Selector de carpeta moderno

const CLSID_FileOpenDialog: GUID = GUID(0xDC1C5A9C, 0xE88A, 0x4DDE, [0xA5, 0xA1, 0x60, 0xF8, 0x2A, 0x20, 0xAE, 0xF7]);
const IID_IFileOpenDialog: GUID = GUID(0xD57C7288, 0xD4AD, 0x4768, [0xBE, 0x02, 0x9D, 0x96, 0x95, 0x32, 0xD9, 0x60]);

fn pick_folder(owner: H) -> Option<PathBuf> {
    unsafe {
        let mut d = null_mut();
        if CoCreateInstance(&CLSID_FileOpenDialog, null_mut(), 0x1, &IID_IFileOpenDialog, &mut d) < 0 {
            return None;
        }
        let get_opts: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT = transmute(vt(d, 10));
        let set_opts: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT = transmute(vt(d, 9));
        let set_title: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT = transmute(vt(d, 17));
        let show: unsafe extern "system" fn(*mut c_void, H) -> HRESULT = transmute(vt(d, 3));
        let get_result: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT = transmute(vt(d, 20));
        let mut o = 0u32;
        get_opts(d, &mut o);
        set_opts(d, o | 0x20 /* PICKFOLDERS */ | 0x40 /* FORCEFILESYSTEM */);
        let title = w("ClipLite — elige dónde guardar los clips y partidas");
        set_title(d, title.as_ptr());
        let mut result = None;
        if show(d, owner) >= 0 {
            let mut item = null_mut();
            if get_result(d, &mut item) >= 0 {
                let name: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u16) -> HRESULT = transmute(vt(item, 5));
                let mut p = null_mut();
                if name(item, 0x8005_8000 /* SIGDN_FILESYSPATH */, &mut p) >= 0 && !p.is_null() {
                    result = Some(PathBuf::from(from_wide_ptr(p)));
                    CoTaskMemFree(p as *mut c_void);
                }
                release(item);
            }
        }
        release(d);
        result
    }
}

// ------------------------------------------------------------------ Interfaz (bandeja del sistema)

#[derive(Clone)]
struct View {
    recording: bool,
    game: String,
    mode: Mode,
    folder: PathBuf,
    encoder: String,
    hk_clip: String,
    hk_rec: String,
}

struct Ctx {
    tx: Mutex<Sender<Cmd>>,
    queue: Mutex<VecDeque<UiMsg>>,
    view: Mutex<View>,
    hwnd: AtomicIsize,
    taskbar_msg: u32,
    icon_idle: H,
    icon_rec: H,
    cfg_path: PathBuf,
    log_path: PathBuf,
}

static CTX: OnceLock<Ctx> = OnceLock::new();
static GAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn ctx() -> &'static Ctx {
    CTX.get().expect("ctx")
}
fn send(c: Cmd) {
    let _ = ctx().tx.lock().unwrap().send(c);
}

/// Icono de 32×32 generado en código: anillo gris (en espera) o punto rojo (grabando).
unsafe fn make_icon(inst: H, rgb: (u8, u8, u8), ring: bool) -> H {
    const N: usize = 32;
    let mut xor = vec![0u8; N * N * 4];
    let and = vec![0u8; N * N / 8];
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let mut a = (14.0 - d).clamp(0.0, 1.0);
            if ring {
                a *= (d - 8.5).clamp(0.0, 1.0);
                a = a.max((5.0 - d).clamp(0.0, 1.0));
            }
            let i = (y * N + x) * 4;
            xor[i] = rgb.2;
            xor[i + 1] = rgb.1;
            xor[i + 2] = rgb.0;
            xor[i + 3] = (a * 255.0) as u8;
        }
    }
    CreateIcon(inst, N as i32, N as i32, 1, 32, and.as_ptr(), xor.as_ptr())
}

unsafe fn tray(op: u32, info: Option<&str>) {
    let c = ctx();
    let v = c.view.lock().unwrap().clone();
    let mut d: NOTIFYICONDATAW = zeroed();
    d.cb_size = size_of::<NOTIFYICONDATAW>() as u32;
    d.hwnd = c.hwnd.load(Ordering::SeqCst);
    d.uid = 1;
    d.flags = 0x1 | 0x2 | 0x4; // MESSAGE | ICON | TIP
    d.callback_msg = WM_TRAY;
    d.icon = if v.recording { c.icon_rec } else { c.icon_idle };
    let tip = if v.recording { format!("ClipLite — grabando {}", v.game) } else { "ClipLite — esperando un juego".into() };
    copy_wide(&mut d.tip, &tip);
    if let Some(msg) = info {
        d.flags |= 0x10; // NIF_INFO
        copy_wide(&mut d.info, msg);
        copy_wide(&mut d.info_title, "ClipLite");
        d.info_flags = 0x1 | 0x10; // NIIF_INFO | NIIF_NOSOUND
    }
    Shell_NotifyIconW(op, &mut d);
}

unsafe fn register_hotkeys(hwnd: H, cfg: &Config) -> Vec<String> {
    let mut failed = Vec::new();
    for (id, s) in [(1, &cfg.hotkey_clip), (2, &cfg.hotkey_record)] {
        UnregisterHotKey(hwnd, id);
        match crate::util::parse_hotkey(s) {
            Some((m, vk)) if RegisterHotKey(hwnd, id, m, vk) != 0 => {}
            _ => failed.push(s.clone()),
        }
    }
    failed
}

unsafe fn show_menu(hwnd: H) {
    let v = ctx().view.lock().unwrap().clone();
    let m = CreatePopupMenu();
    let add = |flags: u32, id: usize, text: &str| {
        AppendMenuW(m, flags, id, w(text).as_ptr());
    };
    let check = |b: bool| if b { MF_CHECKED } else { 0 };
    add(MF_GRAYED, 1, &if v.recording { format!("● Grabando: {}", v.game) } else { "Esperando a que abras un juego…".into() });
    add(MF_GRAYED, 2, &format!("Codificador: {}", v.encoder));
    add(MF_SEPARATOR, 0, "");
    add(0, ID_TOGGLE, &if v.recording { format!("Detener grabación\t{}", v.hk_rec) } else { format!("Empezar a grabar ahora\t{}", v.hk_rec) });
    add(if v.recording { 0 } else { MF_GRAYED }, ID_CLIP, &format!("Guardar clip\t{}", v.hk_clip));
    add(0, ID_LIBRARY, "Ver mis partidas y clips");
    add(MF_SEPARATOR, 0, "");
    add(check(v.mode == Mode::Completa), ID_MODE_FULL, "Grabar la partida completa");
    add(check(v.mode == Mode::Momentos), ID_MODE_MOM, "Solo momentos importantes");
    add(MF_SEPARATOR, 0, "");
    add(0, ID_FOLDER, "Cambiar carpeta de guardado…");
    add(0, ID_OPEN, "Abrir carpeta de clips");
    add(0, ID_EDIT, "Editar configuración");
    add(0, ID_RELOAD, "Recargar configuración");
    add(0, ID_LOG, "Ver registro");
    add(check(autostart_enabled()), ID_AUTOSTART, "Iniciar con Windows");
    add(MF_SEPARATOR, 0, "");
    add(0, ID_QUIT, "Salir");

    let mut p = POINT { x: 0, y: 0 };
    GetCursorPos(&mut p);
    SetForegroundWindow(hwnd);
    let id = TrackPopupMenu(m, 0x2 | 0x100 | 0x80 /* RIGHTBUTTON|RETURNCMD|NONOTIFY */, p.x, p.y, 0, hwnd, null());
    PostMessageW(hwnd, 0, 0, 0);
    DestroyMenu(m);

    let open = |file: &str, params: Option<&str>| {
        let params = params.map(w);
        ShellExecuteW(hwnd, w("open").as_ptr(), w(file).as_ptr(), params.as_ref().map_or(null(), |p| p.as_ptr()), null(), 1);
    };
    match id as usize {
        ID_TOGGLE => send(Cmd::ToggleRecord),
        ID_CLIP => send(Cmd::SaveClip),
        ID_LIBRARY => send(Cmd::OpenLibrary),
        ID_MODE_FULL => send(Cmd::SetMode(Mode::Completa)),
        ID_MODE_MOM => send(Cmd::SetMode(Mode::Momentos)),
        ID_FOLDER => {
            if let Some(p) = pick_folder(hwnd) {
                ctx().view.lock().unwrap().folder = p.clone();
                send(Cmd::SetFolder(p));
            }
        }
        ID_OPEN => {
            let _ = std::fs::create_dir_all(&v.folder);
            open(&v.folder.to_string_lossy(), None);
        }
        ID_EDIT => open("notepad.exe", Some(&format!("\"{}\"", ctx().cfg_path.display()))),
        ID_LOG => open("notepad.exe", Some(&format!("\"{}\"", ctx().log_path.display()))),
        ID_RELOAD => {
            let cfg = Config::load_or_create(&ctx().cfg_path);
            *GAMES.lock().unwrap() = cfg.games.clone();
            let failed = register_hotkeys(hwnd, &cfg);
            {
                let mut vv = ctx().view.lock().unwrap();
                vv.hk_clip = cfg.hotkey_clip.clone();
                vv.hk_rec = cfg.hotkey_record.clone();
            }
            if !failed.is_empty() {
                tray(NIM_MODIFY, Some(&format!("No se pudo usar el atajo {} (¿lo usa otro programa?)", failed.join(", "))));
            }
            send(Cmd::Reload);
        }
        ID_AUTOSTART => set_autostart(!autostart_enabled()),
        ID_QUIT => send(Cmd::Quit),
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: H, msg: u32, wparam: usize, lparam: isize) -> isize {
    let Some(c) = CTX.get() else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_TRAY => {
            let ev = (lparam as u32) & 0xFFFF;
            if ev == WM_RBUTTONUP || ev == WM_LBUTTONUP {
                show_menu(hwnd);
            } else if ev == 0x0203 {
                // doble clic: abrir el reproductor
                send(Cmd::OpenLibrary);
            }
            0
        }
        WM_HOTKEY => {
            match wparam {
                1 => send(Cmd::SaveClip),
                2 => send(Cmd::ToggleRecord),
                _ => {}
            }
            0
        }
        WM_UPDATE => {
            let msgs: Vec<UiMsg> = c.queue.lock().unwrap().drain(..).collect();
            for m in msgs {
                match m {
                    UiMsg::State { recording, game, mode, folder, encoder } => {
                        {
                            let mut v = c.view.lock().unwrap();
                            v.recording = recording;
                            v.game = game;
                            v.mode = mode;
                            v.folder = folder;
                            v.encoder = encoder;
                        }
                        tray(NIM_MODIFY, None);
                    }
                    UiMsg::Notice(s) if s.is_empty() => {
                        MessageBeep(0);
                    }
                    UiMsg::Notice(s) => tray(NIM_MODIFY, Some(&s)),
                    UiMsg::Exit => {
                        DestroyWindow(hwnd);
                    }
                }
            }
            0
        }
        WM_DESTROY => {
            tray(NIM_DELETE, None);
            PostQuitMessage(0);
            0
        }
        m if m == c.taskbar_msg && m != 0 => {
            // El explorador se reinició: volver a poner el icono.
            tray(NIM_ADD, None);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn spawn_game_watcher(tx: Sender<Cmd>) {
    std::thread::Builder::new()
        .name("juegos".into())
        .spawn(move || {
            let mut current: Option<String> = None;
            loop {
                let games = GAMES.lock().unwrap().clone();
                let procs = running_processes();
                let found = games.into_iter().find(|g| {
                    let l = g.to_lowercase();
                    procs.iter().any(|p| *p == l)
                });
                if found != current {
                    if let Some(c) = current.take() {
                        if tx.send(Cmd::GameStopped(c)).is_err() {
                            return;
                        }
                    }
                    if let Some(f) = &found {
                        log!("Juego detectado: {f}");
                        if tx.send(Cmd::GameStarted(f.clone())).is_err() {
                            return;
                        }
                    }
                    current = found;
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        })
        .ok();
}

fn message_box(text: &str, flags: u32) -> i32 {
    unsafe { MessageBoxW(0, w(text).as_ptr(), w("ClipLite").as_ptr(), flags) }
}

pub fn run() {
    unsafe {
        CreateMutexW(null(), 1, w("Local\\ClipLite.UnicaInstancia").as_ptr());
        if GetLastError() == 183 {
            message_box("ClipLite ya está abierto.\nBúscalo en los iconos junto al reloj.", 0x40);
            return;
        }
        // Nuestro proceso cede la CPU al juego. (FFmpeg sigue en prioridad normal.)
        SetPriorityClass(GetCurrentProcess(), 0x4000 /* BELOW_NORMAL */);
        CoInitializeEx(null(), 0x2 /* STA, para el selector de carpetas */);
    }

    let cdir = config_dir();
    let _ = std::fs::create_dir_all(&cdir);
    let log_path = cdir.join("log.txt");
    crate::log::init(&log_path);
    let cfg_path = crate::config::config_path();
    let first_run = !cfg_path.exists();
    let cfg = Config::load_or_create(&cfg_path);
    log!("ClipLite {} iniciado", env!("CARGO_PKG_VERSION"));

    let Some(ffmpeg) = crate::util::find_ffmpeg(&cfg.ffmpeg) else {
        let r = message_box(
            "No se encontró FFmpeg (el motor de captura por GPU).\n\n\
             1. Descarga \"ffmpeg-release-essentials.zip\" (versión 6.1 o superior).\n\
             2. Copia el archivo bin\\ffmpeg.exe a la misma carpeta que ClipLite.exe.\n\
             3. Vuelve a abrir ClipLite.\n\n¿Abrir la página de descarga ahora?",
            0x4 | 0x30,
        );
        if r == 6 {
            unsafe {
                ShellExecuteW(0, w("open").as_ptr(), w("https://www.gyan.dev/ffmpeg/builds/").as_ptr(), null(), null(), 1);
            }
        }
        return;
    };
    log!("FFmpeg: {}", ffmpeg.display());

    unsafe {
        let inst = GetModuleHandleW(null());
        let class = w("ClipLiteVentana");
        let wc = WNDCLASSEXW {
            cb_size: size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            wnd_proc: wndproc,
            cls_extra: 0,
            wnd_extra: 0,
            instance: inst,
            icon: 0,
            cursor: 0,
            background: 0,
            menu_name: null(),
            class_name: class.as_ptr(),
            icon_sm: 0,
        };
        RegisterClassExW(&wc);
        let (tx, rx) = channel::<Cmd>();
        let _ = CTX.set(Ctx {
            tx: Mutex::new(tx.clone()),
            queue: Mutex::new(VecDeque::new()),
            view: Mutex::new(View {
                recording: false,
                game: String::new(),
                mode: cfg.mode,
                folder: cfg.output_dir.clone(),
                encoder: "comprobando…".into(),
                hk_clip: cfg.hotkey_clip.clone(),
                hk_rec: cfg.hotkey_record.clone(),
            }),
            hwnd: AtomicIsize::new(0),
            taskbar_msg: RegisterWindowMessageW(w("TaskbarCreated").as_ptr()),
            icon_idle: make_icon(inst, (150, 156, 164), true),
            icon_rec: make_icon(inst, (229, 57, 53), false),
            cfg_path: cfg_path.clone(),
            log_path,
        });
        let hwnd = CreateWindowExW(0, class.as_ptr(), w("ClipLite").as_ptr(), 0, 0, 0, 0, 0, 0, 0, inst, null_mut());
        if hwnd == 0 {
            message_box("No se pudo crear la ventana de ClipLite.", 0x10);
            return;
        }
        ctx().hwnd.store(hwnd, Ordering::SeqCst);
        tray(NIM_ADD, None);

        let failed = register_hotkeys(hwnd, &cfg);
        if !failed.is_empty() {
            tray(NIM_MODIFY, Some(&format!("No se pudo usar el atajo {} (¿lo usa otro programa?). Cámbialo en la configuración.", failed.join(", "))));
        } else if first_run {
            tray(
                NIM_MODIFY,
                Some(&format!(
                    "Listo. Grabará solo al abrir un juego.\n{} = guardar clip · {} = grabar/parar",
                    cfg.hotkey_clip, cfg.hotkey_record
                )),
            );
        }

        *GAMES.lock().unwrap() = cfg.games.clone();
        spawn_game_watcher(tx.clone());

        let notify: Notify = Arc::new(|m: UiMsg| {
            let c = ctx();
            c.queue.lock().unwrap().push_back(m);
            PostMessageW(c.hwnd.load(Ordering::SeqCst), WM_UPDATE, 0, 0);
        });
        let lol: FetcherFactory = Arc::new(lol_fetcher);
        let controller = Controller::new(cfg, cfg_path, ffmpeg, tx, notify, lol);
        std::thread::Builder::new().name("control".into()).spawn(move || controller.run(rx)).ok();

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    log!("ClipLite cerrado");
}
