// Todo lo específico de Windows, llamando directamente a la API del sistema (sin librerías).
//  * Icono en la bandeja + menú, atajos globales, notificaciones.
//  * Audio del sistema con WASAPI en modo loopback (lo mismo que suena por los altavoces).
//  * Detección de juegos (lista de procesos cada 3 s).
//  * Cliente HTTPS local para la API de League of Legends (WinHTTP).
#![allow(non_snake_case, non_upper_case_globals, clippy::missing_transmute_annotations)]

use crate::audio::{AudioFormat, Capture, TrackSpec, TrackSrc, MULTI};
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
#[derive(Clone, Copy, PartialEq, Eq)]
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
#[repr(C)]
struct SHFILEOPSTRUCTW {
    hwnd: H,
    func: u32,
    from: *const u16,
    to: *const u16,
    flags: u16,
    aborted: BOOL,
    name_mappings: *mut c_void,
    progress_title: *const u16,
}

#[link(name = "shell32")]
extern "system" {
    fn SHFileOperationW(op: *mut SHFILEOPSTRUCTW) -> i32;
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
    fn LoadLibraryW(name: *const u16) -> H;
    fn GetProcAddress(m: H, name: *const u8) -> usize;
    fn CreateEventW(attr: *const c_void, manual: BOOL, initial: BOOL, name: *const u16) -> H;
    fn WaitForSingleObject(h: H, ms: u32) -> u32;
    fn SetEvent(h: H) -> BOOL;
    fn SetThreadPriority(h: H, p: i32) -> BOOL;
    fn GetCurrentThread() -> H;
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
const ID_CLEAN: usize = 50; // 50..=54
const CLEAN_OPTIONS: [(u32, &str); 5] = [(0, "Nunca"), (7, "Más de 7 días"), (14, "Más de 14 días"), (30, "Más de 30 días"), (90, "Más de 90 días")];
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

/// Abre el reproductor como ventana de aplicación (Edge en modo app, incluido en Windows);
/// si no está disponible, con el navegador predeterminado.
pub fn open_url(url: &str) {
    unsafe {
        let args = w(&format!("--app=\"{url}\" --window-size=1440,920"));
        let r = ShellExecuteW(0, w("open").as_ptr(), w("msedge.exe").as_ptr(), args.as_ptr(), null(), 1);
        if r <= 32 {
            ShellExecuteW(0, w("open").as_ptr(), w(url).as_ptr(), null(), null(), 1);
        }
    }
}

/// Envía un archivo a la Papelera de reciclaje (se puede recuperar).
pub fn trash(p: &Path) -> Result<(), String> {
    // SHFileOperation no admite el prefijo \\?\ y exige la lista terminada en doble nulo.
    let path = p.to_string_lossy().trim_start_matches(r"\\?\").to_string();
    let mut from: Vec<u16> = path.encode_utf16().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: 0,
        func: 3, // FO_DELETE
        from: from.as_ptr(),
        to: null(),
        flags: 0x40 | 0x10 | 0x4 | 0x400, // ALLOWUNDO | NOCONFIRMATION | SILENT | NOERRORUI
        aborted: 0,
        name_mappings: null_mut(),
        progress_title: null(),
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if r == 0 && op.aborted == 0 {
        Ok(())
    } else {
        Err(format!("SHFileOperation 0x{r:X}"))
    }
}

/// El hilo de audio: prioridad máxima (solo copia datos cada 10 ms; no quita rendimiento al juego).
pub fn boost_audio_thread() {
    unsafe {
        SetThreadPriority(GetCurrentThread(), 15 /* THREAD_PRIORITY_TIME_CRITICAL */);
    }
}

/// FFmpeg: prioridad de CPU "por encima de lo normal" y prioridad de GPU alta, para que la
/// captura no se quede sin turno cuando el juego usa la GPU al 100 % (lo mismo que hace OBS).
pub fn boost_process(h: H) {
    unsafe {
        SetPriorityClass(h, 0x8000 /* ABOVE_NORMAL_PRIORITY_CLASS */);
        let gdi = LoadLibraryW(w("gdi32.dll").as_ptr());
        if gdi != 0 {
            let f = GetProcAddress(gdi, b"D3DKMTSetProcessSchedulingPriorityClass\0".as_ptr());
            if f != 0 {
                let set: unsafe extern "system" fn(H, i32) -> i32 = transmute(f);
                let r = set(h, 4 /* D3DKMT_SCHEDULINGPRIORITYCLASS_HIGH */);
                if r != 0 {
                    log!("No se pudo subir la prioridad de GPU de FFmpeg (0x{:08X})", r as u32);
                }
            }
        }
    }
}

/// Abre el Explorador con el archivo seleccionado.
pub fn reveal(p: &Path) {
    let path = p.to_string_lossy().trim_start_matches(r"\\?\").to_string();
    let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn();
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

/// (pid, pid del padre, nombre en minúsculas) de cada proceso.
fn process_list() -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(0x2, 0);
        if snap == -1 {
            return out;
        }
        let mut e: PROCESSENTRY32W = zeroed();
        e.size = size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                let n = e.exe_file.iter().position(|&c| c == 0).unwrap_or(260);
                out.push((e.pid, e.parent_pid, String::from_utf16_lossy(&e.exe_file[..n]).to_lowercase()));
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

/// Proceso principal de una aplicación: el que no tiene como padre a otro proceso del mismo nombre
/// (Discord, Chrome… abren muchos; capturando el principal se incluyen todos sus hijos).
fn root_pid(procs: &[(u32, u32, String)], exe: &str) -> Option<u32> {
    let exe = exe.to_lowercase();
    let same: Vec<&(u32, u32, String)> = procs.iter().filter(|p| p.2 == exe).collect();
    same.iter().find(|p| !same.iter().any(|q| q.0 == p.1)).or(same.first()).map(|p| p.0)
}

fn app_name(exe: &str) -> String {
    let base = exe.trim_end_matches(".exe").trim_end_matches(".EXE");
    let mut c = base.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => exe.to_string(),
    }
}

/// Pistas para esta partida: mezcla completa + juego + apps configuradas abiertas + micrófono.
pub fn track_specs(cfg: &Config, game_exe: &str) -> Vec<TrackSpec> {
    let mut v = vec![TrackSpec { name: "Todo".into(), src: TrackSrc::System }];
    if cfg.app_tracks {
        let procs = process_list();
        if let Some(pid) = root_pid(&procs, game_exe) {
            v.push(TrackSpec { name: "Juego".into(), src: TrackSrc::Process(pid) });
        }
        for app in &cfg.track_apps {
            if app.eq_ignore_ascii_case(game_exe) {
                continue;
            }
            if let Some(pid) = root_pid(&procs, app) {
                v.push(TrackSpec { name: app_name(app), src: TrackSrc::Process(pid) });
            }
        }
    }
    if cfg.mic {
        v.push(TrackSpec { name: "Micrófono".into(), src: TrackSrc::Mic });
    }
    v
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

// ------------------------------------------------------------------ WASAPI

const CLSID_MMDeviceEnumerator: GUID = GUID(0xBCDE0395, 0xE52F, 0x467C, [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E]);
const IID_IMMDeviceEnumerator: GUID = GUID(0xA95664D2, 0x9614, 0x4F35, [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6]);
const IID_IAudioClient: GUID = GUID(0x1CB9AD4C, 0xDBFA, 0x4C32, [0xB1, 0x78, 0xC2, 0xF5, 0x68, 0xA7, 0x03, 0xB2]);
const IID_IAudioCaptureClient: GUID = GUID(0xC8ADBD64, 0xE71E, 0x48A0, [0xA4, 0xDE, 0x18, 0x5C, 0x39, 0x5C, 0xD3, 0x17]);
const IID_IUnknown: GUID = GUID(0x00000000, 0x0000, 0x0000, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IAgileObject: GUID = GUID(0x94EA2B94, 0xE9CC, 0x49E0, [0xC0, 0xFF, 0xEE, 0x64, 0xCA, 0x8F, 0x5B, 0x90]);
const IID_IActivateCompletion: GUID = GUID(0x41D949AB, 0x9862, 0x444A, [0x80, 0xF6, 0xC2, 0x61, 0x33, 0x4D, 0xA5, 0xEB]);

const LOOPBACK: u32 = 0x0002_0000;
const EVENTCALLBACK: u32 = 0x0004_0000;
const AUTOCONVERT: u32 = 0x8000_0000 | 0x0800_0000; // AUTOCONVERTPCM | SRC_DEFAULT_QUALITY

/// WAVEFORMATEX de 18 bytes.
fn waveformat(tag: u16, rate: u32, channels: u16, bits: u16) -> [u8; 18] {
    let align = channels * bits / 8;
    let mut f = [0u8; 18];
    f[0..2].copy_from_slice(&tag.to_le_bytes());
    f[2..4].copy_from_slice(&channels.to_le_bytes());
    f[4..8].copy_from_slice(&rate.to_le_bytes());
    f[8..12].copy_from_slice(&(rate * align as u32).to_le_bytes());
    f[12..14].copy_from_slice(&align.to_le_bytes());
    f[14..16].copy_from_slice(&bits.to_le_bytes());
    f
}

struct Wasapi {
    client: *mut c_void,
    capture: *mut c_void,
    /// Formato que entrega `read` (tras convertir).
    fmt: AudioFormat,
    /// Bytes por fotograma tal y como llegan de Windows.
    raw_bpf: usize,
    /// Llega en PCM de 16 bits y se convierte a float.
    s16: bool,
    event: H,
}

impl Drop for Wasapi {
    fn drop(&mut self) {
        unsafe {
            let stop: unsafe extern "system" fn(*mut c_void) -> HRESULT = transmute(vt(self.client, 11));
            stop(self.client);
            release(self.capture);
            release(self.client);
            if self.event != 0 {
                CloseHandle(self.event);
            }
        }
    }
}

type InitFn = unsafe extern "system" fn(*mut c_void, i32, u32, i64, i64, *const u8, *const GUID) -> HRESULT;

/// Inicializa un IAudioClient ya activado, obtiene el capturador y lo arranca.
unsafe fn start_client(client: *mut c_void, fmt_bytes: &[u8], flags: u32, fmt: AudioFormat, raw_bpf: usize, s16: bool) -> Result<Wasapi, String> {
    let init: InitFn = transmute(vt(client, 3));
    let buffer = if flags & EVENTCALLBACK != 0 { 2_000_000 } else { 10_000_000 };
    hr(init(client, 0, flags, buffer, 0, fmt_bytes.as_ptr(), null()), "Initialize")?;
    let mut event = 0;
    if flags & EVENTCALLBACK != 0 {
        // Obligatorio con EVENTCALLBACK; no lo esperamos, leemos por sondeo.
        event = CreateEventW(null(), 0, 0, null());
        let set_event: unsafe extern "system" fn(*mut c_void, H) -> HRESULT = transmute(vt(client, 13));
        hr(set_event(client, event), "SetEventHandle")?;
    }
    let get_service: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT = transmute(vt(client, 14));
    let mut capture = null_mut();
    hr(get_service(client, &IID_IAudioCaptureClient, &mut capture), "IAudioCaptureClient")?;
    let start: unsafe extern "system" fn(*mut c_void) -> HRESULT = transmute(vt(client, 10));
    if let Err(e) = hr(start(client), "Start") {
        // El cliente lo libera quien llamó; aquí solo lo que se creó en esta función.
        release(capture);
        if event != 0 {
            CloseHandle(event);
        }
        return Err(e);
    }
    Ok(Wasapi { client, capture, fmt, raw_bpf, s16, event })
}

/// Activa el IAudioClient del dispositivo predeterminado de salida (flow 0) o de entrada (flow 1).
unsafe fn endpoint_client(flow: i32) -> Result<*mut c_void, String> {
    let mut en = null_mut();
    hr(CoCreateInstance(&CLSID_MMDeviceEnumerator, null_mut(), 0x17, &IID_IMMDeviceEnumerator, &mut en), "MMDeviceEnumerator")?;
    let get_default: unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> HRESULT = transmute(vt(en, 4));
    let mut dev = null_mut();
    let r = get_default(en, flow, 0, &mut dev);
    release(en);
    hr(r, if flow == 0 { "no hay dispositivo de salida de audio" } else { "no hay micrófono" })?;
    let activate: unsafe extern "system" fn(*mut c_void, *const GUID, u32, *mut c_void, *mut *mut c_void) -> HRESULT = transmute(vt(dev, 3));
    let mut client = null_mut();
    let r = activate(dev, &IID_IAudioClient, 0x17, null_mut(), &mut client);
    release(dev);
    hr(r, "IAudioClient")?;
    Ok(client)
}

/// Salida del sistema (loopback) o micrófono. Con `multi` se pide 48 kHz estéreo float
/// (Windows convierte si hace falta); si no, el formato nativo del dispositivo.
unsafe fn open_endpoint(flow: i32, multi: bool) -> Result<Wasapi, String> {
    let loop_flag = if flow == 0 { LOOPBACK } else { 0 };
    if multi {
        let client = endpoint_client(flow)?;
        let f = waveformat(3, 48_000, 2, 32);
        match start_client(client, &f, loop_flag | AUTOCONVERT, MULTI, 8, false) {
            Ok(wsp) => return Ok(wsp),
            Err(e) => {
                release(client);
                if flow != 0 {
                    return Err(e);
                }
                log!("Audio: 48 kHz estéreo no disponible ({e}); se usa el formato del dispositivo");
            }
        }
    }
    let client = endpoint_client(flow)?;
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
    let size = 18 + rd16(16) as usize;
    let bytes = std::slice::from_raw_parts(wf, size).to_vec();
    CoTaskMemFree(wf as *mut c_void);
    match start_client(client, &bytes, loop_flag, fmt, fmt.bytes_per_frame(), false) {
        Ok(wsp) => Ok(wsp),
        Err(e) => {
            release(client);
            Err(e)
        }
    }
}

// --- Captura de una sola aplicación (Windows 10 2004+/Windows 11) ---

#[repr(C)]
struct ActivationParams {
    activation_type: u32, // 1 = PROCESS_LOOPBACK
    pid: u32,
    mode: u32, // 0 = incluir el proceso y sus hijos
}

#[repr(C)]
struct PropVariantBlob {
    vt: u16, // VT_BLOB = 65
    r1: u16,
    r2: u16,
    r3: u16,
    cb: u32,
    pad: u32,
    data: *const u8,
}

/// Objeto COM mínimo que recibe el aviso de "activación terminada".
#[repr(C)]
struct Handler {
    vtbl: *const HandlerVtbl,
    event: H,
    refs: std::sync::atomic::AtomicU32,
}
#[repr(C)]
struct HandlerVtbl {
    qi: unsafe extern "system" fn(*mut Handler, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut Handler) -> u32,
    release: unsafe extern "system" fn(*mut Handler) -> u32,
    completed: unsafe extern "system" fn(*mut Handler, *mut c_void) -> HRESULT,
}
static HANDLER_VTBL: HandlerVtbl = HandlerVtbl { qi: h_qi, add_ref: h_add_ref, release: h_release, completed: h_completed };

unsafe extern "system" fn h_qi(this: *mut Handler, riid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    let id = *riid;
    if id == IID_IUnknown || id == IID_IActivateCompletion || id == IID_IAgileObject {
        *out = this as *mut c_void;
        h_add_ref(this);
        0
    } else {
        *out = null_mut();
        0x8000_4002u32 as i32 // E_NOINTERFACE
    }
}
unsafe extern "system" fn h_add_ref(this: *mut Handler) -> u32 {
    (*this).refs.fetch_add(1, Ordering::SeqCst) + 1
}
unsafe extern "system" fn h_release(this: *mut Handler) -> u32 {
    let n = (*this).refs.fetch_sub(1, Ordering::SeqCst) - 1;
    if n == 0 {
        CloseHandle((*this).event);
        drop(Box::from_raw(this));
    }
    n
}
unsafe extern "system" fn h_completed(this: *mut Handler, _op: *mut c_void) -> HRESULT {
    SetEvent((*this).event);
    0
}

type ActivateFn = unsafe extern "system" fn(*const u16, *const GUID, *const PropVariantBlob, *mut c_void, *mut *mut c_void) -> HRESULT;

unsafe fn open_process(pid: u32) -> Result<Wasapi, String> {
    // Primero 48 kHz float; si no, PCM 16 bits (el formato del ejemplo oficial) y se convierte.
    // Un IAudioClient no se puede reinicializar tras un fallo: para el segundo intento se activa otro.
    let client = open_process_client(pid)?;
    let f32fmt = waveformat(3, 48_000, 2, 32);
    match start_client(client, &f32fmt, LOOPBACK | EVENTCALLBACK | AUTOCONVERT, MULTI, 8, false) {
        Ok(wsp) => Ok(wsp),
        Err(e1) => {
            release(client);
            let client = open_process_client(pid)?;
            let s16fmt = waveformat(1, 48_000, 2, 16);
            start_client(client, &s16fmt, LOOPBACK | EVENTCALLBACK, MULTI, 4, true).map_err(|e2| {
                release(client);
                format!("{e1}; {e2}")
            })
        }
    }
}

/// Activa otro IAudioClient para el mismo proceso (sin inicializar).
unsafe fn open_process_client(pid: u32) -> Result<*mut c_void, String> {
    // Se carga en tiempo de ejecución: en Windows antiguos simplemente no hay pistas por aplicación.
    let lib = LoadLibraryW(w("mmdevapi.dll").as_ptr());
    let f = if lib != 0 { GetProcAddress(lib, b"ActivateAudioInterfaceAsync\0".as_ptr()) } else { 0 };
    if f == 0 {
        return Err("esta versión de Windows no permite capturar una sola aplicación".into());
    }
    let activate: ActivateFn = transmute(f);
    let params = ActivationParams { activation_type: 1, pid, mode: 0 };
    let pv = PropVariantBlob { vt: 65, r1: 0, r2: 0, r3: 0, cb: size_of::<ActivationParams>() as u32, pad: 0, data: &params as *const _ as *const u8 };
    let event = CreateEventW(null(), 1, 0, null());
    let handler = Box::into_raw(Box::new(Handler { vtbl: &HANDLER_VTBL, event, refs: std::sync::atomic::AtomicU32::new(1) }));
    let mut op = null_mut();
    let r = activate(w("VAD\\Process_Loopback").as_ptr(), &IID_IAudioClient, &pv, handler as *mut c_void, &mut op);
    if r < 0 {
        h_release(handler);
        return Err(format!("ActivateAudioInterfaceAsync (0x{:08X}): requiere Windows 10 2004 o superior", r as u32));
    }
    let ok = WaitForSingleObject(event, 5000) == 0;
    let (mut result, mut unk): (HRESULT, *mut c_void) = (-1, null_mut());
    if ok {
        let get_result: unsafe extern "system" fn(*mut c_void, *mut HRESULT, *mut *mut c_void) -> HRESULT = transmute(vt(op, 3));
        get_result(op, &mut result, &mut unk);
    }
    release(op);
    h_release(handler);
    hr(if ok { result } else { -1 }, "activar captura de la aplicación")?;
    let qi: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT = transmute(vt(unk, 0));
    let mut client = null_mut();
    let r = qi(unk, &IID_IAudioClient, &mut client);
    release(unk);
    hr(r, "IAudioClient de la aplicación")?;
    Ok(client)
}

/// Abre una fuente de audio. Con `multi`, todas entregan 48 kHz estéreo float.
pub fn open_source(src: &TrackSrc, multi: bool) -> Result<Box<dyn Capture>, String> {
    unsafe {
        CoInitializeEx(null(), 0 /* COINIT_MULTITHREADED */);
        let wsp = match src {
            TrackSrc::System => open_endpoint(0, multi)?,
            TrackSrc::Mic => open_endpoint(1, true)?,
            TrackSrc::Process(pid) => open_process(*pid)?,
        };
        let f = wsp.fmt;
        if *src == TrackSrc::System {
            log!("Audio: {} Hz, {} canales, {} bits{}", f.rate, f.channels, f.bits, if f.float { " float" } else { "" });
        }
        Ok(Box::new(wsp))
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
            loop {
                let mut n = 0u32;
                hr(next(self.capture, &mut n), "GetNextPacketSize")?;
                if n == 0 {
                    return Ok(());
                }
                let (mut data, mut frames, mut flags) = (null_mut(), 0u32, 0u32);
                hr(get(self.capture, &mut data, &mut frames, &mut flags, null_mut(), null_mut()), "GetBuffer")?;
                let raw = frames as usize * self.raw_bpf;
                let silent = flags & 0x2 != 0 || data.is_null();
                if self.s16 {
                    if silent {
                        out.resize(out.len() + frames as usize * 8, 0);
                    } else {
                        for c in std::slice::from_raw_parts(data, raw).chunks_exact(2) {
                            let v = i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0;
                            out.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                } else if silent {
                    out.resize(out.len() + raw, 0); // AUDCLNT_BUFFERFLAGS_SILENT
                } else {
                    out.extend_from_slice(std::slice::from_raw_parts(data, raw));
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
    clean_days: u32,
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
    let sub = CreatePopupMenu();
    for (i, (d, label)) in CLEAN_OPTIONS.iter().enumerate() {
        AppendMenuW(sub, check(v.clean_days == *d), ID_CLEAN + i, w(label).as_ptr());
    }
    AppendMenuW(sub, MF_SEPARATOR, 0, w("").as_ptr());
    AppendMenuW(sub, MF_GRAYED, 3, w("Los favoritos nunca se borran").as_ptr());
    add(0x10 /* MF_POPUP */, sub as usize, "Borrar vídeos viejos automáticamente");
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
        x if (ID_CLEAN..ID_CLEAN + CLEAN_OPTIONS.len()).contains(&x) => send(Cmd::SetCleanDays(CLEAN_OPTIONS[x - ID_CLEAN].0)),
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
                    UiMsg::State { recording, game, mode, folder, encoder, clean_days } => {
                        {
                            let mut v = c.view.lock().unwrap();
                            v.recording = recording;
                            v.game = game;
                            v.mode = mode;
                            v.folder = folder;
                            v.encoder = encoder;
                            v.clean_days = clean_days;
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
                clean_days: cfg.clean_days,
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
