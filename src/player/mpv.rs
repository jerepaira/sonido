//! Backend libmpv (FFI a mano, sin crates intermedias).
//!
//! Por qué mpv: decodifica FLAC 24/192, ALAC, AAC, Opus y **Dolby Atmos**
//! (E-AC-3 J-OC / AC-4) con downmix automático al dispositivo, y hace gapless
//! real entre entradas adyacentes de su playlist interna.

use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use anyhow::{anyhow, Result};

use crate::player::{AudioInfo, Backend, PlayRequest, PlayerEvent, Slot};

// ────────────────────────────────── FFI ──────────────────────────────────

type MpvHandle = c_void;

pub const MPV_FORMAT_NONE: u32 = 0;
pub const MPV_FORMAT_STRING: u32 = 1;
pub const MPV_FORMAT_FLAG: u32 = 3;
pub const MPV_FORMAT_INT64: u32 = 4;
pub const MPV_FORMAT_DOUBLE: u32 = 5;

pub const MPV_EVENT_SHUTDOWN: i32 = 1;
pub const MPV_EVENT_LOG_MESSAGE: i32 = 2;
pub const MPV_EVENT_END_FILE: i32 = 7;
pub const MPV_EVENT_FILE_LOADED: i32 = 8;
pub const MPV_EVENT_PLAYBACK_RESTART: i32 = 21;
pub const MPV_EVENT_PROPERTY_CHANGE: i32 = 22;

pub const MPV_END_FILE_REASON_EOF: i32 = 0;
pub const MPV_END_FILE_REASON_STOP: i32 = 2;
pub const MPV_END_FILE_REASON_QUIT: i32 = 3;
pub const MPV_END_FILE_REASON_ERROR: i32 = 4;
pub const MPV_END_FILE_REASON_REDIRECT: i32 = 5;

/// reply_userdata de las propiedades observadas
const PROP_TIME_POS: u64 = 1001;
const PROP_PAUSE: u64 = 1002;
const PROP_AUDIO_PARAMS: u64 = 1003;
const PROP_DURATION: u64 = 1004;
const PROP_CACHE_BUFFERED: u64 = 1005;
const PROP_PLAYLIST_COUNT: u64 = 1006;

#[repr(C)]
pub struct MpvEvent {
    pub event_id: i32,
    pub error: i32,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct MpvEventProperty {
    pub name: *const c_char,
    pub format: u32,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct MpvEventEndFile {
    pub reason: i32,
    pub error: i32,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: i32,
}

#[repr(C)]
pub struct MpvEventLogMessage {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
}

extern "C" {
    fn mpv_create() -> *mut MpvHandle;
    fn mpv_initialize(ctx: *mut MpvHandle) -> c_int;
    fn mpv_terminate_destroy(ctx: *mut MpvHandle);
    fn mpv_command(ctx: *mut MpvHandle, args: *const *const c_char) -> c_int;
    fn mpv_set_option_string(
        ctx: *mut MpvHandle,
        name: *const c_char,
        data: *const c_char,
    ) -> c_int;
    fn mpv_set_property_string(
        ctx: *mut MpvHandle,
        name: *const c_char,
        data: *const c_char,
    ) -> c_int;
    fn mpv_set_property(
        ctx: *mut MpvHandle,
        name: *const c_char,
        format: u32,
        data: *const c_void,
    ) -> c_int;
    fn mpv_get_property(
        ctx: *mut MpvHandle,
        name: *const c_char,
        format: u32,
        data: *mut c_void,
    ) -> c_int;
    fn mpv_observe_property(
        ctx: *mut MpvHandle,
        reply_userdata: u64,
        name: *const c_char,
        format: u32,
    ) -> c_int;
    fn mpv_wait_event(ctx: *mut MpvHandle, timeout: c_double) -> *mut MpvEvent;
    fn mpv_error_string(error: c_int) -> *const c_char;
}

fn err_str(code: c_int) -> String {
    unsafe {
        let p = mpv_error_string(code);
        if p.is_null() {
            format!("mpv error {code}")
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

/// Convierte un `&str` en `*const c_char` cacheándolo por hilo.
///
/// La memoria se retiene a propósito (nunca se libera): el conjunto de strings
/// distintos es acotado (nombres de propiedades, comandos, headers) y mpv copia
/// el contenido de forma síncrona en todas las APIs que usamos. Esto evita un
/// `CString` nuevo por llamada en el hot path del event loop.
fn cstr(s: &str) -> *const c_char {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static CACHE: RefCell<HashMap<String, *const c_char>> =
            RefCell::new(HashMap::new());
    }

    CACHE.with(|c| {
        let mut map = c.borrow_mut();
        if let Some(p) = map.get(s) {
            return *p;
        }
        let p = CString::new(s).unwrap().into_raw() as *const c_char;
        map.insert(s.to_string(), p);
        p
    })
}

struct CArgs {
    _strings: Vec<CString>,
    ptrs: Vec<*const c_char>,
}

impl CArgs {
    fn new(args: &[&str]) -> Self {
        let strings: Vec<CString> = args
            .iter()
            .map(|a| CString::new(*a).unwrap_or_else(|_| CString::new("").unwrap()))
            .collect();
        let mut ptrs: Vec<*const c_char> = strings.iter().map(|s| s.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        Self {
            _strings: strings,
            ptrs,
        }
    }
    fn as_ptr(&self) -> *const *const c_char {
        self.ptrs.as_ptr()
    }
}

// ──────────────────────────────── estado compartido ────────────────────────────────

#[derive(Debug)]
struct Shared {
    position: AtomicU64, // f64 como bits
    duration: AtomicU64,
    paused: AtomicBool,
    running: AtomicBool,
    audio: RwLock<Option<AudioInfo>>,
    buffered: AtomicU64,
    playlist_count: AtomicI64,
    current: RwLock<Option<Slot>>,
    /// Instante del último EOF (para medir el gap real de la transición).
    eof_at: RwLock<Option<Instant>>,
    /// URL del archivo actual, para que los errores digan qué falló.
    current_url: RwLock<String>,
    /// Generación de reproducción: se incrementa con cada `play()` nuevo, así el
    /// hilo de eventos descarta eventos de archivos anteriores.
    generation: AtomicU64,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            position: AtomicU64::new(0),
            duration: AtomicU64::new(0),
            paused: AtomicBool::new(true),
            running: AtomicBool::new(false),
            audio: RwLock::new(None),
            buffered: AtomicU64::new(0),
            playlist_count: AtomicI64::new(0),
            current: RwLock::new(None),
            eof_at: RwLock::new(None),
            current_url: RwLock::new(String::new()),
            generation: AtomicU64::new(0),
        }
    }
}

fn bits_to_f64(v: u64) -> f64 {
    f64::from_bits(v)
}

// ──────────────────────────────── backend ────────────────────────────────

pub struct MpvBackend {
    handle: std::sync::Mutex<*mut MpvHandle>,
    shared: Arc<Shared>,
    tx: tokio::sync::mpsc::UnboundedSender<PlayerEvent>,
    volume: AtomicU64,
    muted: AtomicBool,
    speed: AtomicU64,
    _event_thread: Option<std::thread::JoinHandle<()>>,
}

/// Wrapper para poder mover el handle entre hilos: el client API de mpv
/// garantiza que todas sus funciones son thread-safe.
#[derive(Clone, Copy)]
struct MpvPtr(*mut MpvHandle);
unsafe impl Send for MpvPtr {}
unsafe impl Sync for MpvPtr {}

// Todas las funciones del client API de mpv son thread-safe por contrato.
unsafe impl Send for MpvBackend {}
unsafe impl Sync for MpvBackend {}

impl MpvBackend {
    /// Crea el handle y arranca el hilo de eventos.
    pub fn with_sender(tx: tokio::sync::mpsc::UnboundedSender<PlayerEvent>) -> Result<Self> {
        unsafe {
            let h = mpv_create();
            if h.is_null() {
                return Err(anyhow!("mpv_create devolvió NULL"));
            }
            set_opt(h, "vid", "no");
            set_opt(h, "audio-display", "no");
            set_opt(h, "keep-open", "no");
            set_opt(h, "idle", "yes");
            set_opt(h, "gapless-audio", "yes");
            set_opt(h, "cache", "yes");
            set_opt(h, "demuxer-readahead-secs", "60");
            set_opt(h, "demuxer-max-bytes", "512MiB");
            set_opt(h, "network-timeout", "30");
            set_opt(h, "audio-buffer", "0.4");
            set_opt(h, "replaygain", "no");
            set_opt(h, "volume", "100");
            set_opt(h, "volume-max", "150");
            set_opt(h, "terminal", "no");
            set_opt(h, "input-default-bindings", "no");
            set_opt(h, "osc", "no");
            set_opt(h, "config", "no");
            set_opt(h, "msg-level", "all=warn");
            // Algunos CDN (CloudFront de Tidal, por caso) rechazan el UA por
            // defecto de mpv/ffmpeg. Usamos el mismo que para las peticiones.
            set_opt(h, "user-agent", crate::http::USER_AGENT);
            set_opt(h, "tls-verify", "yes");

            let rc = mpv_initialize(h);
            if rc < 0 {
                let e = err_str(rc);
                mpv_terminate_destroy(h);
                return Err(anyhow!("mpv_initialize falló: {e}"));
            }

            mpv_observe_property(h, PROP_TIME_POS, cstr("time-pos"), MPV_FORMAT_DOUBLE);
            mpv_observe_property(h, PROP_DURATION, cstr("duration"), MPV_FORMAT_DOUBLE);
            mpv_observe_property(h, PROP_PAUSE, cstr("pause"), MPV_FORMAT_FLAG);
            mpv_observe_property(h, PROP_AUDIO_PARAMS, cstr("audio-params"), MPV_FORMAT_STRING);
            mpv_observe_property(
                h,
                PROP_CACHE_BUFFERED,
                cstr("cache-buffered-state"),
                MPV_FORMAT_DOUBLE,
            );
            mpv_observe_property(
                h,
                PROP_PLAYLIST_COUNT,
                cstr("playlist-count"),
                MPV_FORMAT_INT64,
            );

            let shared = Arc::new(Shared::default());
            let ev_shared = shared.clone();
            let ev_tx = tx.clone();
            let ev_ptr = MpvPtr(h);
            let th = std::thread::Builder::new()
                .name("mpv-events".into())
                .spawn(move || event_loop(ev_ptr, ev_shared, ev_tx))
                .map_err(|e| anyhow!("no se pudo crear el hilo de eventos: {e}"))?;

            Ok(Self {
                handle: std::sync::Mutex::new(h),
                shared,
                tx,
                volume: AtomicU64::new(1.0f64.to_bits()),
                muted: AtomicBool::new(false),
                speed: AtomicU64::new(1.0f64.to_bits()),
                _event_thread: Some(th),
            })
        }
    }

    pub fn sender(&self) -> tokio::sync::mpsc::UnboundedSender<PlayerEvent> {
        self.tx.clone()
    }

    fn handle(&self) -> *mut MpvHandle {
        *self.handle.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn cmd(&self, args: &[&str]) -> Result<()> {
        let c = CArgs::new(args);
        let rc = unsafe { mpv_command(self.handle(), c.as_ptr()) };
        if rc < 0 {
            Err(anyhow!("comando mpv {args:?} falló: {}", err_str(rc)))
        } else {
            Ok(())
        }
    }

    fn set_prop(&self, name: &str, value: &str) -> Result<()> {
        let rc = unsafe { mpv_set_property_string(self.handle(), cstr(name), cstr(value)) };
        if rc < 0 {
            tracing::debug!("set {name}={value} -> {}", err_str(rc));
        }
        Ok(())
    }

    fn set_flag(&self, name: &str, on: bool) {
        let v: c_int = if on { 1 } else { 0 };
        unsafe {
            mpv_set_property(
                self.handle(),
                cstr(name),
                MPV_FORMAT_FLAG,
                &v as *const c_int as *const c_void,
            );
        }
    }

    fn set_double(&self, name: &str, v: f64) {
        let d: c_double = v;
        unsafe {
            mpv_set_property(
                self.handle(),
                cstr(name),
                MPV_FORMAT_DOUBLE,
                &d as *const c_double as *const c_void,
            );
        }
    }

    fn get_double(&self, name: &str) -> Option<f64> {
        let mut d: c_double = 0.0;
        let rc = unsafe {
            mpv_get_property(
                self.handle(),
                cstr(name),
                MPV_FORMAT_DOUBLE,
                &mut d as *mut c_double as *mut c_void,
            )
        };
        if rc >= 0 && d.is_finite() && d >= 0.0 {
            Some(d)
        } else {
            None
        }
    }

    fn get_i64(&self, name: &str) -> Option<i64> {
        let mut v: i64 = 0;
        let rc = unsafe {
            mpv_get_property(
                self.handle(),
                cstr(name),
                MPV_FORMAT_INT64,
                &mut v as *mut i64 as *mut c_void,
            )
        };
        if rc >= 0 {
            Some(v)
        } else {
            None
        }
    }

    fn get_string(&self, name: &str) -> Option<String> {
        let mut p: *mut c_char = std::ptr::null_mut();
        let rc = unsafe {
            mpv_get_property(
                self.handle(),
                cstr(name),
                MPV_FORMAT_STRING,
                &mut p as *mut *mut c_char as *mut c_void,
            )
        };
        if rc < 0 || p.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        Some(s)
    }

    /// Aplica las opciones que pide el addon (si existen en mpv; si no, se ignoran).
    fn apply_options(&self, opts: &[(String, String)]) {
        for (k, v) in opts {
            let _ = self.set_prop(k, v);
        }
    }

    /// Opciones de carga según el tipo de manifiesto.
    ///
    /// DASH/HLS con URLs firmadas: el archivo no es un audio sino un XML con
    /// segmentos. mpv lo reproduce vía ffmpeg, pero hay que decírselo
    /// explícitamente porque la extensión queda tapada por el query string
    /// (`.../manifests/xxx.mpd?Expires=…&Signature=…`).
    fn load_opts(&self, adaptive: crate::resolve::Adaptive) -> Vec<String> {
        use crate::resolve::Adaptive;
        match adaptive {
            Adaptive::Dash => vec![
                "demuxer=dash".to_string(),
                // el .mpd es un manifiesto: no tiene sentido pedirle Range
                "stream-lavf-o=seekable=0".to_string(),
            ],
            Adaptive::Hls => vec!["demuxer=hls".to_string()],
            Adaptive::None => Vec::new(),
        }
    }

    fn apply_headers(&self, headers: &[(String, String)]) {
        if headers.is_empty() {
            let _ = self.set_prop("http-header-fields", "");
            return;
        }
        let joined: Vec<String> = headers
            .iter()
            .map(|(k, v)| format!("{k}: {}", v.replace(['\r', '\n', '"'], "")))
            .collect();
        let _ = self.set_prop("http-header-fields", &joined.join(","));
    }

    /// Posición actual dentro de la playlist de mpv (-1 si no hay nada).
    fn playing_pos(&self) -> i64 {
        self.get_i64("playlist-playing-pos").unwrap_or(-1)
    }
}

fn set_opt(h: *mut MpvHandle, name: &str, value: &str) {
    let rc = unsafe { mpv_set_option_string(h, cstr(name), cstr(value)) };
    if rc < 0 {
        tracing::debug!("opción mpv {name}={value} rechazada: {}", err_str(rc));
    }
}

// ──────────────────────────────── event loop ────────────────────────────────

fn event_loop(
    ptr: MpvPtr,
    shared: Arc<Shared>,
    tx: tokio::sync::mpsc::UnboundedSender<PlayerEvent>,
) {
    let h = ptr.0;
    loop {
        let ev = unsafe { mpv_wait_event(h, 0.2) };
        if ev.is_null() {
            continue;
        }
        let ev = unsafe { &*ev };
        match ev.event_id {
            MPV_EVENT_SHUTDOWN => break,

            MPV_EVENT_LOG_MESSAGE => {
                if tracing::enabled!(tracing::Level::TRACE) && !ev.data.is_null() {
                    let l = unsafe { &*(ev.data as *const MpvEventLogMessage) };
                    let lvl = unsafe { cstr_to_string(l.level) };
                    if lvl == "error" || lvl == "fatal" {
                        tracing::trace!(
                            "mpv[{}]: {}",
                            unsafe { cstr_to_string(l.prefix) },
                            unsafe { cstr_to_string(l.text) }.trim()
                        );
                    }
                }
            }

            MPV_EVENT_PLAYBACK_RESTART => {
                shared.running.store(true, Ordering::Relaxed);
                shared.paused.store(false, Ordering::Relaxed);
                // gap real de la transición (EOF -> primer frame del siguiente)
                if let Ok(mut g) = shared.eof_at.write() {
                    if let Some(at) = g.take() {
                        let gap = at.elapsed().as_millis() as u64;
                        let _ = tx.send(PlayerEvent::Gap(gap));
                    }
                }
                let slot = *shared.current.read().unwrap_or_else(|e| e.into_inner());
                if let Some(slot) = slot {
                    let _ = tx.send(PlayerEvent::Playing { slot });
                }
            }

            MPV_EVENT_END_FILE => {
                if ev.data.is_null() {
                    continue;
                }
                let e = unsafe { &*(ev.data as *const MpvEventEndFile) };
                match e.reason {
                    MPV_END_FILE_REASON_EOF => {
                        *shared.eof_at.write().unwrap_or_else(|p| p.into_inner()) =
                            Some(Instant::now());
                        shared.running.store(false, Ordering::Relaxed);
                        // ¿mpv ya arrancó el siguiente (gapless) o se quedó sin playlist?
                        let next_pos = get_i64_at(h, "playlist-playing-pos").unwrap_or(-1);
                        let slot = *shared.current.read().unwrap_or_else(|p| p.into_inner());
                        let slot = slot.unwrap_or(Slot::A);
                        let _ = tx.send(PlayerEvent::Ended { slot });
                        if next_pos <= e.playlist_entry_id {
                            // no quedaba nada pre-cargado: mpv se fue a idle
                            shared.paused.store(true, Ordering::Relaxed);
                            let _ = tx.send(PlayerEvent::Idle);
                        }
                    }
                    MPV_END_FILE_REASON_ERROR => {
                        shared.running.store(false, Ordering::Relaxed);
                        let url = shared
                            .current_url
                            .read()
                            .map(|u| u.clone())
                            .unwrap_or_default();
                        let base = if e.error != 0 {
                            err_str(e.error)
                        } else {
                            "error de reproducción".into()
                        };
                        let msg = if url.is_empty() {
                            base
                        } else {
                            format!("{base} — {}", crate::util::ellipsize(&url, 110))
                        };
                        let slot = *shared.current.read().unwrap_or_else(|p| p.into_inner());
                        let _ = tx.send(PlayerEvent::Failed {
                            slot: slot.unwrap_or(Slot::A),
                            error: msg,
                        });
                    }
                    MPV_END_FILE_REASON_STOP | MPV_END_FILE_REASON_QUIT => {
                        shared.running.store(false, Ordering::Relaxed);
                        shared.paused.store(true, Ordering::Relaxed);
                    }
                    MPV_END_FILE_REASON_REDIRECT => {}
                    _ => {}
                }
            }

            MPV_EVENT_PROPERTY_CHANGE => {
                if ev.data.is_null() {
                    continue;
                }
                let p = unsafe { &*(ev.data as *const MpvEventProperty) };
                match ev.reply_userdata {
                    PROP_TIME_POS => {
                        if p.format == MPV_FORMAT_DOUBLE && !p.data.is_null() {
                            let v = unsafe { *(p.data as *const c_double) };
                            if v.is_finite() && v >= 0.0 {
                                shared.position.store(v.to_bits(), Ordering::Relaxed);
                                let _ = tx.send(PlayerEvent::Position(v));
                            }
                        }
                    }
                    PROP_DURATION => {
                        if p.format == MPV_FORMAT_DOUBLE && !p.data.is_null() {
                            let v = unsafe { *(p.data as *const c_double) };
                            if v.is_finite() && v > 0.0 {
                                shared.duration.store(v.to_bits(), Ordering::Relaxed);
                                let _ = tx.send(PlayerEvent::Duration(v));
                            }
                        }
                    }
                    PROP_PAUSE => {
                        if p.format == MPV_FORMAT_FLAG && !p.data.is_null() {
                            let v = unsafe { *(p.data as *const c_int) } != 0;
                            shared.paused.store(v, Ordering::Relaxed);
                            let _ = tx.send(PlayerEvent::Paused(v));
                        }
                    }
                    PROP_CACHE_BUFFERED => {
                        if p.format == MPV_FORMAT_DOUBLE && !p.data.is_null() {
                            let v = unsafe { *(p.data as *const c_double) };
                            if v.is_finite() {
                                shared.buffered.store(v.to_bits(), Ordering::Relaxed);
                                let _ = tx.send(PlayerEvent::Buffering(v.clamp(0.0, 1.0)));
                            }
                        }
                    }
                    PROP_PLAYLIST_COUNT => {
                        if p.format == MPV_FORMAT_INT64 && !p.data.is_null() {
                            let v = unsafe { *(p.data as *const i64) };
                            shared.playlist_count.store(v, Ordering::Relaxed);
                        }
                    }
                    PROP_AUDIO_PARAMS => {
                        if p.format == MPV_FORMAT_STRING && !p.data.is_null() {
                            let raw = unsafe { *(p.data as *const *const c_char) };
                            let txt = unsafe { cstr_to_string(raw) };
                            let info = parse_audio_params(&txt);
                            if let Ok(mut g) = shared.audio.write() {
                                if g.as_ref() != Some(&info) {
                                    *g = Some(info.clone());
                                    let _ = tx.send(PlayerEvent::AudioInfo(info));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            MPV_EVENT_FILE_LOADED => {}
            _ => {}
        }
    }
    tracing::debug!("hilo de eventos de mpv finalizado");
}

fn get_i64_at(h: *mut MpvHandle, name: &str) -> Option<i64> {
    let mut v: i64 = 0;
    let rc = unsafe {
        mpv_get_property(
            h,
            cstr(name),
            MPV_FORMAT_INT64,
            &mut v as *mut i64 as *mut c_void,
        )
    };
    if rc >= 0 {
        Some(v)
    } else {
        None
    }
}

unsafe fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

/// `audio-params` -> "format: s32, samplerate: 192000, channels: stereo, ..."
fn parse_audio_params(s: &str) -> AudioInfo {
    let mut info = AudioInfo::default();
    let mut chan_str = String::new();
    for part in s.split(',') {
        let mut it = part.splitn(2, ':');
        let (Some(k), Some(v)) = (it.next(), it.next()) else {
            continue;
        };
        match k.trim() {
            "format" => {
                let f = v.trim();
                // mpv reporta el formato de muestra, no el codec del archivo
                info.codec = match f {
                    "u8" | "s16" | "s16p" | "s24" | "s24p" | "s32" | "s32p" | "flt" | "fltp" => {
                        "pcm".to_string()
                    }
                    other => other.to_string(),
                };
            }
            "samplerate" => info.sample_rate = v.trim().parse().unwrap_or(0),
            "channels" => chan_str = v.trim().to_string(),
            _ => {}
        }
    }
    info.channels = channels_of(&chan_str);
    let fmt = s
        .split(',')
        .find_map(|p| p.trim().strip_prefix("format:"))
        .unwrap_or("")
        .trim();
    info.bit_depth = match fmt {
        "u8" => 8,
        "s16" | "s16p" => 16,
        "s24" | "s24p" | "s32" | "s32p" => 24,
        "flt" | "fltp" | "s64" | "dbl" | "dblp" => 32,
        _ => 0,
    };
    if info.codec == "pcm" && info.bit_depth == 0 {
        info.bit_depth = 16;
    }
    info
}

fn channels_of(s: &str) -> u32 {
    match s {
        "mono" => 1,
        "stereo" | "" => 2,
        "2.1" => 3,
        "3.0" | "3.0(back)" => 3,
        "4.0" => 4,
        "5.0" | "5.0(side)" => 5,
        "5.1" | "5.1(side)" => 6,
        "6.0" => 6,
        "7.0" => 7,
        "7.1" | "7.1(wide)" => 8,
        "7.1(rear)" => 8,
        _ => s.matches('.').count() as u32 + 2,
    }
}

// ──────────────────────────────── Backend impl ────────────────────────────────

impl Backend for MpvBackend {
    fn name(&self) -> &'static str {
        "libmpv"
    }

    fn play(&self, slot: Slot, req: PlayRequest) -> Result<()> {
        let h = self.handle();
        let _ = self.cmd(&["stop"]);
        let _ = self.cmd(&["playlist-clear"]);
        self.shared.generation.fetch_add(1, Ordering::Relaxed);
        self.shared.position.store(0, Ordering::Relaxed);
        self.shared.duration.store(0, Ordering::Relaxed);
        *self.shared.current.write().unwrap_or_else(|e| e.into_inner()) = Some(slot);
        *self.shared.eof_at.write().unwrap_or_else(|e| e.into_inner()) = None;

        *self.shared.current_url.write().unwrap_or_else(|e| e.into_inner()) =
            req.url.clone();
        self.apply_headers(&req.headers);
        self.apply_options(&req.options);
        let start = if req.start_at > 0.05 {
            Some(format!("start=+{:.3}", req.start_at))
        } else {
            None
        };
        let mut joined_opts = self.load_opts(req.adaptive);
        for (k, v) in &req.options {
            joined_opts.push(format!("{k}={v}"));
        }
        let start: Option<String> = match start {
            Some(s) => {
                let mut v = vec![s];
                v.extend(joined_opts.iter().cloned());
                Some(v.join(","))
            }
            None => (!joined_opts.is_empty()).then(|| joined_opts.join(",")),
        };
        let args: Vec<&str> = match &start {
            Some(s) => vec!["loadfile", req.url.as_str(), "replace", s.as_str()],
            None => vec!["loadfile", req.url.as_str(), "replace"],
        };
        let joined_dbg = args.join(" ");
        let c = CArgs::new(&args);
        let rc = unsafe { mpv_command(h, c.as_ptr()) };
        // restaurar headers vacíos para no filtrarlos al preload
        self.apply_headers(&[]);
        if rc < 0 {
            return Err(anyhow!("loadfile falló ({joined_dbg}): {}", err_str(rc)));
        }
        self.set_flag("pause", false);
        Ok(())
    }

    fn preload(&self, slot: Slot, req: PlayRequest) -> Result<()> {
        self.apply_headers(&req.headers);
        self.apply_options(&req.options);
        *self.shared.current_url.write().unwrap_or_else(|e| e.into_inner()) =
            req.url.clone();
        let args = ["loadfile", req.url.as_str(), "append-play"];
        let c = CArgs::new(&args);
        let rc = unsafe { mpv_command(self.handle(), c.as_ptr()) };
        self.apply_headers(&[]);
        if rc < 0 {
            return Err(anyhow!("preload falló: {}", err_str(rc)));
        }
        tracing::debug!("preload slot {} -> {}", slot.label(), req.url);
        Ok(())
    }

    fn promote(&self, slot: Slot) {
        *self.shared.current.write().unwrap_or_else(|e| e.into_inner()) = Some(slot);
        self.shared.position.store(0, Ordering::Relaxed);
        self.shared.duration.store(0, Ordering::Relaxed);
        *self.shared.audio.write().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn pause(&self, on: bool) {
        self.set_flag("pause", on);
        self.shared.paused.store(on, Ordering::Relaxed);
    }

    fn toggle_pause(&self) -> bool {
        let cur = self.is_paused();
        self.pause(!cur);
        !cur
    }

    fn stop(&self) {
        let _ = self.cmd(&["stop"]);
        let _ = self.cmd(&["playlist-clear"]);
        self.shared.running.store(false, Ordering::Relaxed);
        self.shared.paused.store(true, Ordering::Relaxed);
        self.shared.position.store(0, Ordering::Relaxed);
        *self.shared.current.write().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn seek(&self, secs: f64) -> Option<f64> {
        let s = format!("{:.3}", secs.max(0.0));
        let _ = self.cmd(&["seek", &s, "absolute"]);
        self.shared.position.store(secs.max(0.0).to_bits(), Ordering::Relaxed);
        Some(secs.max(0.0))
    }

    fn position(&self) -> f64 {
        bits_to_f64(self.shared.position.load(Ordering::Relaxed))
    }

    fn duration(&self) -> f64 {
        let d = bits_to_f64(self.shared.duration.load(Ordering::Relaxed));
        if d > 0.0 {
            d
        } else {
            self.get_double("duration").unwrap_or(0.0)
        }
    }

    fn set_volume(&self, v: f32) {
        let v = v.clamp(0.0, 1.5);
        self.volume.store((v as f64).to_bits(), Ordering::Relaxed);
        self.set_double("volume", (v * 100.0) as f64);
    }

    fn volume(&self) -> f32 {
        f64::from_bits(self.volume.load(Ordering::Relaxed)) as f32
    }

    fn set_muted(&self, m: bool) {
        self.muted.store(m, Ordering::Relaxed);
        self.set_flag("mute", m);
    }

    fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    fn set_speed(&self, s: f32) {
        let s = s.clamp(0.25, 4.0);
        self.speed.store((s as f64).to_bits(), Ordering::Relaxed);
        self.set_double("speed", s as f64);
    }

    fn speed(&self) -> f32 {
        f64::from_bits(self.speed.load(Ordering::Relaxed)) as f32
    }

    fn set_replaygain(&self, mode: &str) {
        let m = match mode {
            "track" => "track",
            "album" => "album",
            _ => "no",
        };
        let _ = self.set_prop("replaygain", m);
    }

    fn audio_info(&self) -> Option<AudioInfo> {
        self.shared.audio.read().ok()?.clone()
    }

    fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    fn current_slot(&self) -> Option<Slot> {
        *self.shared.current.read().unwrap_or_else(|e| e.into_inner())
    }

    fn audio_devices(&self) -> Vec<(String, String)> {
        let raw = self.get_string("audio-device-list").unwrap_or_default();
        parse_device_list(&raw)
    }

    fn set_audio_device(&self, id: &str) {
        let _ = self.set_prop("audio-device", if id.is_empty() { "auto" } else { id });
    }

    fn set_exclusive(&self, on: bool) {
        self.set_flag("audio-exclusive", on);
    }

    fn play_video(&self, url: &str) -> Result<()> {
        let _ = self.set_prop("vid", "auto");
        let _ = self.set_prop("force-window", "yes");
        self.cmd(&["loadfile", url, "replace"])
    }

    fn stop_video(&self) {
        let _ = self.set_prop("vid", "no");
        let _ = self.set_prop("force-window", "no");
    }

    fn backend_stats(&self) -> String {
        let buffered = bits_to_f64(self.shared.buffered.load(Ordering::Relaxed));
        let count = self.shared.playlist_count.load(Ordering::Relaxed);
        let codec = self.get_string("audio-codec").unwrap_or_else(|| "-".into());
        let dev = self.get_string("audio-device-name")
            .or_else(|| self.get_string("audio-device"))
            .unwrap_or_else(|| "-".into());
        let cache = self
            .get_double("demuxer-cache-time")
            .map(|s| format!("{s:.0}s"))
            .unwrap_or_else(|| "-".into());
        format!("entrada {}/{} · buffer {buffered:.0}% · cache {cache} · {codec} · {dev}",
            self.playing_pos() + 1, count)
    }
}

impl Drop for MpvBackend {
    fn drop(&mut self) {
        let h = *self.handle.get_mut().unwrap_or_else(|e| e.into_inner());
        if !h.is_null() {
            unsafe { mpv_terminate_destroy(h) };
        }
    }
}

/// `audio-device-list` -> "{name} (desc), {name2} (desc2)"
fn parse_device_list(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(open) = rest.find('{') {
        let close = match rest[open..].find('}') {
            Some(c) => open + c,
            None => break,
        };
        let block = &rest[open + 1..close];
        let mut name = String::new();
        let mut desc = String::new();
        for kv in block.split(", ") {
            if let Some(v) = kv.strip_prefix("name=") {
                name = v.trim_matches('"').to_string();
            } else if let Some(v) = kv.strip_prefix("description=") {
                desc = v.trim_matches('"').to_string();
            }
        }
        if !name.is_empty() {
            let label = if desc.is_empty() { name.clone() } else { desc };
            out.push((name, label));
        }
        rest = &rest[close + 1..];
    }
    out
}
