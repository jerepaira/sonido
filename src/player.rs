//! Motor de reproducción: cola, gapless y backends (libmpv / rodio).

pub mod fallback;
#[cfg(mpv)]
pub mod mpv;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::Result;

use crate::models::LibraryTrack;
use crate::resolve::ResolvedStream;

// ──────────────────────────────── eventos ────────────────────────────────

#[derive(Debug, Clone)]
pub enum PlayerEvent {
    /// Empezó a sonar un slot concreto.
    Playing { slot: Slot },
    Paused(bool),
    /// El slot actual terminó (EOF). Con gapless esto ocurre después de que el
    /// backend ya arrancó el siguiente.
    Ended { slot: Slot },
    /// Falló la reproducción del slot.
    Failed { slot: Slot, error: String },
    Position(f64),
    Duration(f64),
    AudioInfo(AudioInfo),
    Buffering(f64),
    /// Gap medido (ms) entre el EOF de un tema y el primer frame del siguiente.
    Gap(u64),
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Slot::A => "A",
            Slot::B => "B",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub bit_depth: u32,
    pub bitrate_kbps: u32,
}

impl AudioInfo {
    pub fn badge(&self) -> String {
        let mut v = Vec::new();
        let codec = if self.codec.is_empty() {
            if self.bit_depth > 0 || self.sample_rate > 0 {
                "PCM".to_string()
            } else {
                String::new()
            }
        } else {
            self.codec.to_ascii_uppercase()
        };
        if !codec.is_empty() {
            v.push(codec);
        }
        if self.bit_depth > 0 && self.sample_rate > 0 {
            v.push(format!("{}/{}k", self.bit_depth, self.sample_rate / 1000));
        } else if self.bitrate_kbps > 0 {
            v.push(format!("{}k", self.bitrate_kbps));
        }
        if self.channels > 2 {
            v.push(format!("{}ch", self.channels));
        }
        v.join(" ")
    }
}

// ──────────────────────────────── backend ────────────────────────────────

#[derive(Debug, Clone)]
pub struct PlayRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub start_at: f64,
    pub title: String,
    /// Opciones que el addon pide pasarle al motor (demuxer, probesize, …).
    /// Vienen de `mpvOptions`/`playerOptions`/`ffmpegOptions` en la respuesta de /stream.
    pub options: Vec<(String, String)>,
    /// La URL es un manifiesto DASH/HLS en vez de un archivo de audio.
    pub adaptive: crate::resolve::Adaptive,
}

pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;

    /// Arranca `slot` y deja el otro libre.
    fn play(&self, slot: Slot, req: PlayRequest) -> Result<()>;

    /// Pre-carga el siguiente stream en el slot libre -> transición gapless.
    fn preload(&self, slot: Slot, req: PlayRequest) -> Result<()>;

    /// Promueve el slot pre-cargado a "actual" (lo usa el gapless de mpv).
    fn promote(&self, slot: Slot);

    fn pause(&self, on: bool);
    fn toggle_pause(&self) -> bool;
    fn stop(&self);
    /// Devuelve la posición real si el backend la conoce.
    fn seek(&self, secs: f64) -> Option<f64>;
    fn position(&self) -> f64;
    fn duration(&self) -> f64;
    fn set_volume(&self, v: f32);
    fn volume(&self) -> f32;
    fn set_muted(&self, m: bool);
    fn muted(&self) -> bool;
    fn set_speed(&self, s: f32);
    fn speed(&self) -> f32;
    fn set_replaygain(&self, mode: &str);
    fn audio_info(&self) -> Option<AudioInfo>;
    fn is_paused(&self) -> bool;
    fn current_slot(&self) -> Option<Slot>;
    /// Nombre del dispositivo de salida activo + lista de disponibles.
    fn audio_devices(&self) -> Vec<(String, String)>;
    fn set_audio_device(&self, id: &str);
    fn set_exclusive(&self, on: bool);
    /// Reproduce el clip de vídeo asociado (si el backend puede).
    fn play_video(&self, _url: &str) -> Result<()> {
        Err(anyhow::anyhow!("este motor no reproduce vídeo"))
    }
    fn stop_video(&self) {}
    fn backend_stats(&self) -> String {
        String::new()
    }
}

// ──────────────────────────────── cola ────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repeat {
    Off,
    All,
    One,
}

impl Repeat {
    pub fn parse(s: &str) -> Self {
        match s {
            "all" => Repeat::All,
            "one" => Repeat::One,
            _ => Repeat::Off,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }
    pub fn next(&self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
    pub fn icon(&self) -> &'static str {
        match self {
            Repeat::Off => "↺",
            Repeat::All => "∞",
            Repeat::One => "1",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueueItem {
    pub addon_id: String,
    pub addon_name: String,
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub artwork: Option<String>,
    pub isrc: Option<String>,
    pub duration: Option<f64>,
    pub explicit: bool,
    pub hi_res: bool,
}

impl QueueItem {
    pub fn new(addon: &crate::addon::Addon, t: &crate::models::Track) -> Self {
        Self {
            addon_id: addon.id(),
            addon_name: addon.name(),
            track_id: t.id.clone(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            artwork: t.artwork_url.clone(),
            isrc: t.isrc.clone(),
            duration: t.duration_secs(),
            explicit: t.explicit == Some(true),
            hi_res: t.hi_res == Some(true),
        }
    }

    pub fn from_library(t: &LibraryTrack, addon_name: &str) -> Self {
        Self {
            addon_id: t.addon_id.clone(),
            addon_name: addon_name.to_string(),
            track_id: t.track_id.clone(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            artwork: t.artwork_url.clone(),
            isrc: t.isrc.clone(),
            duration: t.duration,
            explicit: false,
            hi_res: false,
        }
    }

    pub fn from_catalog(addon: &crate::addon::Addon, c: &crate::models::CatalogItem) -> Self {
        Self {
            addon_id: addon.id(),
            addon_name: addon.name(),
            track_id: c.id.clone(),
            title: c.title.clone(),
            artist: c.artist.clone(),
            album: c.album.clone(),
            artwork: c.artwork_url.clone(),
            isrc: c.isrc.clone(),
            duration: c.duration_secs(),
            explicit: c.explicit == Some(true),
            hi_res: c.hi_res == Some(true),
        }
    }

    pub fn library(&self) -> LibraryTrack {
        LibraryTrack {
            addon_id: self.addon_id.clone(),
            track_id: self.track_id.clone(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            artwork_url: self.artwork.clone(),
            isrc: self.isrc.clone(),
            duration: self.duration,
            source_url: None,
            added_at: crate::config::now_secs(),
        }
    }

    pub fn identity(&self) -> crate::addon::TrackIdentity {
        crate::addon::TrackIdentity {
            title: self.title.clone(),
            artist: self.artist.clone(),
            isrc: self.isrc.clone(),
            duration: self.duration,
        }
    }

    pub fn key(&self) -> String {
        format!("{}::{}", self.addon_id, self.track_id)
    }

    pub fn subtitle(&self) -> String {
        match &self.album {
            Some(a) if !a.is_empty() => format!("{} · {}", self.artist, a),
            _ => self.artist.clone(),
        }
    }
}

/// Cola de reproducción con shuffle y repeat.
#[derive(Debug)]
pub struct Queue {
    items: RwLock<Vec<QueueItem>>,
    /// Orden de reproducción (índices). Con shuffle es una permutación.
    order: RwLock<Vec<usize>>,
    cursor: RwLock<usize>,
    shuffle: AtomicBool,
    repeat: RwLock<Repeat>,
    version: AtomicU64,
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Queue {
    pub fn new() -> Self {
        Self {
            items: RwLock::new(Vec::new()),
            order: RwLock::new(Vec::new()),
            cursor: RwLock::new(0),
            shuffle: AtomicBool::new(false),
            repeat: RwLock::new(Repeat::Off),
            version: AtomicU64::new(0),
        }
    }

    fn rd<T, F: FnOnce(&Vec<T>) -> R, R>(l: &RwLock<Vec<T>>, f: F) -> R {
        let g = l.read().unwrap_or_else(|e| e.into_inner());
        f(&g)
    }

    fn wr<T, F: FnOnce(&mut Vec<T>) -> R, R>(l: &RwLock<Vec<T>>, f: F) -> R {
        let mut g = l.write().unwrap_or_else(|e| e.into_inner());
        f(&mut g)
    }

    pub fn bump(&self) {
        self.version.fetch_add(1, Ordering::Relaxed);
    }
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    pub fn len(&self) -> usize {
        Self::rd(&self.items, |v| v.len())
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reemplaza toda la cola y deja `start_index` sonando.
    pub fn set(&self, items: Vec<QueueItem>, start_index: usize) {
        let n = items.len();
        let shuf = self.shuffle.load(Ordering::Relaxed);
        let mut order: Vec<usize> = (0..n).collect();
        if shuf {
            shuffle_in_place(&mut order, start_index);
        }
        let cursor = order.iter().position(|&i| i == start_index).unwrap_or(0);
        Self::wr(&self.items, |v| {
            *v = items;
        });
        Self::wr(&self.order, |v| *v = order);
        *self.cursor.write().unwrap_or_else(|e| e.into_inner()) = cursor;
        self.bump();
    }

    pub fn append(&self, items: Vec<QueueItem>) {
        let base = self.len();
        Self::wr(&self.items, |v| v.extend(items.iter().cloned()));
        let n = items.len();
        Self::wr(&self.order, |v| v.extend(base..base + n));
        self.bump();
    }

    pub fn insert_next(&self, item: QueueItem) {
        let pos = {
            let c = *self.cursor.read().unwrap_or_else(|e| e.into_inner());
            c + 1
        };
        let idx = self.len();
        Self::wr(&self.items, |v| v.push(item));
        Self::wr(&self.order, |v| {
            if pos >= v.len() {
                v.push(idx);
            } else {
                v.insert(pos, idx);
            }
        });
        self.bump();
    }

    pub fn remove(&self, order_pos: usize) {
        let removed = Self::wr(&self.order, |v| {
            if order_pos < v.len() {
                Some(v.remove(order_pos))
            } else {
                None
            }
        });
        if let Some(item_idx) = removed {
            Self::wr(&self.items, |v| {
                if item_idx < v.len() {
                    v.remove(item_idx);
                }
            });
            // reindexar el orden (los índices > item_idx bajan 1)
            Self::wr(&self.order, |v| {
                for i in v.iter_mut() {
                    if *i > item_idx {
                        *i -= 1;
                    }
                }
            });
            let mut c = self.cursor.write().unwrap_or_else(|e| e.into_inner());
            if *c >= order_pos && *c > 0 {
                *c -= 1;
            }
        }
        self.bump();
    }

    pub fn clear(&self) {
        Self::wr(&self.items, |v| v.clear());
        Self::wr(&self.order, |v| v.clear());
        *self.cursor.write().unwrap_or_else(|e| e.into_inner()) = 0;
        self.bump();
    }

    pub fn items(&self) -> Vec<QueueItem> {
        Self::rd(&self.items, |v| v.clone())
    }

    /// Items en orden de reproducción.
    pub fn ordered(&self) -> Vec<(usize, QueueItem)> {
        let items = Self::rd(&self.items, |v| v.clone());
        let order = Self::rd(&self.order, |v| v.clone());
        order
            .into_iter()
            .filter(|&i| i < items.len())
            .map(|i| (i, items[i].clone()))
            .collect()
    }

    pub fn cursor(&self) -> usize {
        *self.cursor.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn current(&self) -> Option<QueueItem> {
        let c = self.cursor();
        let order = Self::rd(&self.order, |v| v.clone());
        let items = Self::rd(&self.items, |v| v.clone());
        order.get(c).and_then(|&i| items.get(i).cloned())
    }

    pub fn next_item(&self) -> Option<QueueItem> {
        let order = Self::rd(&self.order, |v| v.clone());
        let items = Self::rd(&self.items, |v| v.clone());
        let c = self.cursor();
        match self.repeat() {
            Repeat::One => order.get(c).and_then(|&i| items.get(i).cloned()),
            _ => order.get(c + 1).and_then(|&i| items.get(i).cloned()),
        }
    }

    pub fn prev_item(&self) -> Option<QueueItem> {
        let order = Self::rd(&self.order, |v| v.clone());
        let items = Self::rd(&self.items, |v| v.clone());
        let c = self.cursor();
        order.get(c.saturating_sub(1)).and_then(|&i| items.get(i).cloned())
    }

    /// Avanza el cursor. Devuelve `false` si se terminó la cola.
    pub fn advance(&self) -> bool {
        let n = Self::rd(&self.order, |v| v.len());
        if n == 0 {
            return false;
        }
        let mut c = self.cursor.write().unwrap_or_else(|e| e.into_inner());
        if *c + 1 >= n {
            match self.repeat() {
                Repeat::All => *c = 0,
                _ => return false,
            }
        } else {
            *c += 1;
        }
        drop(c);
        self.bump();
        true
    }

    pub fn rewind(&self) -> bool {
        let n = Self::rd(&self.order, |v| v.len());
        if n == 0 {
            return false;
        }
        let mut c = self.cursor.write().unwrap_or_else(|e| e.into_inner());
        if *c == 0 {
            match self.repeat() {
                Repeat::All => *c = n - 1,
                _ => return false,
            }
        } else {
            *c -= 1;
        }
        drop(c);
        self.bump();
        true
    }

    pub fn jump(&self, order_pos: usize) -> bool {
        let n = Self::rd(&self.order, |v| v.len());
        if order_pos >= n {
            return false;
        }
        *self.cursor.write().unwrap_or_else(|e| e.into_inner()) = order_pos;
        self.bump();
        true
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle.load(Ordering::Relaxed)
    }

    pub fn set_shuffle(&self, on: bool) {
        if self.shuffle.load(Ordering::Relaxed) == on {
            return;
        }
        self.shuffle.store(on, Ordering::Relaxed);
        let n = self.len();
        if n == 0 {
            return;
        }
        let current_index = self
            .current_index()
            .unwrap_or(0);
        let mut order: Vec<usize> = (0..n).collect();
        if on {
            shuffle_in_place(&mut order, current_index);
        }
        let cursor = order.iter().position(|&i| i == current_index).unwrap_or(0);
        Self::wr(&self.order, |v| *v = order);
        *self.cursor.write().unwrap_or_else(|e| e.into_inner()) = cursor;
        self.bump();
    }

    /// Índice dentro de `items` del tema actual.
    pub fn current_index(&self) -> Option<usize> {
        let c = self.cursor();
        Self::rd(&self.order, |v| v.get(c).copied())
    }

    pub fn repeat(&self) -> Repeat {
        self.repeat.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_repeat(&self, r: Repeat) {
        *self.repeat.write().unwrap_or_else(|e| e.into_inner()) = r;
        self.bump();
    }

    pub fn total_duration(&self) -> f64 {
        Self::rd(&self.items, |v| v.iter().filter_map(|i| i.duration).sum())
    }
}

/// Shuffle determinista-ish (xorshift) que deja `keep` en la primera posición.
fn shuffle_in_place(order: &mut Vec<usize>, keep: usize) {
    let mut seed: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15);
    let mut rng = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let n = order.len();
    if n < 2 {
        return;
    }
    // poner `keep` primero
    if let Some(p) = order.iter().position(|&i| i == keep) {
        order.swap(0, p);
    }
    for i in (1..n).rev() {
        let j = 1 + (rng() as usize % i);
        order.swap(i, j);
    }
}

// ──────────────────────────────── Player ────────────────────────────────

/// Estado compartido del reproductor, accesible desde UI y workers.
#[derive(Debug, Clone, Default)]
pub struct PlayerState {
    pub playing: bool,
    pub paused: bool,
    pub position: f64,
    pub duration: f64,
    pub volume: f32,
    pub muted: bool,
    pub speed: f32,
    pub slot: Option<Slot>,
    pub preloaded: bool,
    pub audio: Option<AudioInfo>,
    pub buffering: Option<f64>,
    pub backend: String,
}

pub struct Player {
    pub backend: Arc<dyn Backend>,
    pub queue: Arc<Queue>,
    pub state: Arc<RwLock<PlayerState>>,
    events: Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<PlayerEvent>>>,
    tx: tokio::sync::mpsc::UnboundedSender<PlayerEvent>,
    /// Stream ya resuelto del siguiente tema (para gapless).
    prerolled: Arc<Mutex<Option<(String, ResolvedStream)>>>,
    stats: Arc<PlayerStats>,
}

#[derive(Debug, Default)]
pub struct PlayerStats {
    pub gap_ms: AtomicU64,
    pub transitions: AtomicU64,
    pub errors: AtomicU64,
    pub last_transition_ms: AtomicU64,
}

impl Player {
    /// Crea el reproductor eligiendo el backend disponible.
    pub fn create() -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (backend, name) = create_backend(tx.clone());
        let st = PlayerState {
            backend: name,
            ..Default::default()
        };
        Self {
            backend,
            queue: Arc::new(Queue::new()),
            state: Arc::new(RwLock::new(st)),
            events: Arc::new(Mutex::new(rx)),
            tx,
            prerolled: Arc::new(Mutex::new(None)),
            stats: Arc::new(PlayerStats::default()),
        }
    }

    pub fn backend_name(&self) -> String {
        self.state.read().map(|s| s.backend.clone()).unwrap_or_default()
    }

    pub fn tx(&self) -> tokio::sync::mpsc::UnboundedSender<PlayerEvent> {
        self.tx.clone()
    }

    /// Saca los eventos pendientes (se llama una vez por frame).
    pub fn drain_events(&self) -> Vec<PlayerEvent> {
        let mut out = Vec::new();
        if let Ok(mut rx) = self.events.lock() {
            while let Ok(e) = rx.try_recv() {
                out.push(e);
            }
        }
        out
    }

    pub fn state(&self) -> PlayerState {
        self.state.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// Mutador genérico del estado compartido (tolerante a poison).
    pub fn with_state<R>(&self, f: impl FnOnce(&mut PlayerState) -> R) -> R {
        let mut g = self.state.write().unwrap_or_else(|e| e.into_inner());
        f(&mut g)
    }

    pub fn set_slot(&self, slot: Slot) {
        self.with_state(|s| s.slot = Some(slot));
    }

    pub fn set_position(&self, p: f64) {
        self.with_state(|s| s.position = p);
    }

    pub fn set_duration(&self, d: f64) {
        if d > 0.0 {
            self.with_state(|s| s.duration = d);
        }
    }

    pub fn set_paused(&self, p: bool) {
        self.with_state(|s| {
            s.paused = p;
            s.playing = !p && s.slot.is_some();
        });
    }

    pub fn set_audio_info(&self, a: AudioInfo) {
        self.with_state(|s| s.audio = Some(a));
    }

    pub fn set_buffering(&self, b: f64) {
        self.with_state(|s| s.buffering = Some(b));
    }

    pub fn set_playing(&self, p: bool) {
        self.with_state(|s| s.playing = p);
    }

    /// Slot donde se pre-cargó el siguiente tema.
    pub fn preroll_slot(&self) -> Slot {
        match self.state().slot {
            Some(Slot::A) => Slot::B,
            _ => Slot::B,
        }
    }

    pub fn stats(&self) -> Arc<PlayerStats> {
        self.stats.clone()
    }

    /// Reproduce un stream ya resuelto (reinicia la cadena de slots).
    pub fn play_resolved(&self, item: &QueueItem, res: &ResolvedStream, start_at: f64) {
        let req = PlayRequest {
            url: res.url.clone(),
            headers: res.headers.clone(),
            start_at,
            title: format!("{} - {}", item.artist, item.title),
            options: res.stream.player_options(),
            adaptive: res.adaptive,
        };
        let slot = Slot::A;
        match self.backend.play(slot, req) {
            Ok(()) => {
                let mut st = self.state.write().unwrap_or_else(|e| e.into_inner());
                st.playing = true;
                st.paused = false;
                st.slot = Some(slot);
                st.preloaded = false;
                st.position = start_at;
                st.duration = item.duration.unwrap_or(0.0);
                st.buffering = None;
                if let Some(v) = res.stream.bit_depth {
                    st.audio = Some(AudioInfo {
                        codec: res
                            .stream
                            .codec
                            .clone()
                            .or_else(|| res.stream.format.clone())
                            .unwrap_or_default(),
                        sample_rate: res.stream.sample_rate.unwrap_or(0),
                        channels: res.stream.channels.unwrap_or(2),
                        bit_depth: v,
                        bitrate_kbps: res.stream.bitrate.unwrap_or(0) / 1000,
                    });
                } else {
                    st.audio = None;
                }
            }
            Err(e) => {
                self.stats.errors.fetch_add(1, Ordering::Relaxed);
                let _ = self.tx.send(PlayerEvent::Failed {
                    slot,
                    error: e.to_string(),
                });
            }
        }
        if let Ok(mut p) = self.prerolled.lock() {
            *p = None;
        }
    }

    /// Deja el siguiente tema pre-cargado para la transición gapless.
    pub fn preroll(&self, key: String, res: ResolvedStream) {
        if self.state().preloaded {
            return;
        }
        let slot = self.preroll_slot();
        let req = PlayRequest {
            url: res.url.clone(),
            headers: res.headers.clone(),
            start_at: 0.0,
            title: res.track_id.clone(),
            options: res.stream.player_options(),
            adaptive: res.adaptive,
        };
        if self.backend.preload(slot, req).is_ok() {
            if let Ok(mut p) = self.prerolled.lock() {
                *p = Some((key, res));
            }
            let mut st = self.state.write().unwrap_or_else(|e| e.into_inner());
            st.preloaded = true;
        }
    }

    pub fn take_prerolled(&self) -> Option<(String, ResolvedStream)> {
        self.prerolled.lock().ok().and_then(|mut p| p.take())
    }

    pub fn has_preroll(&self) -> bool {
        self.prerolled
            .lock()
            .map(|p| p.is_some())
            .unwrap_or(false)
    }

    pub fn promote_prerolled(&self) {
        let slot = match self.state().slot {
            Some(s) => s.other(),
            None => Slot::A,
        };
        self.backend.promote(slot);
        let mut st = self.state.write().unwrap_or_else(|e| e.into_inner());
        st.slot = Some(slot);
        st.preloaded = false;
        st.position = 0.0;
    }

    pub fn toggle_pause(&self) {
        let paused = self.backend.toggle_pause();
        let mut st = self.state.write().unwrap_or_else(|e| e.into_inner());
        st.paused = paused;
        st.playing = !paused && st.slot.is_some();
    }

    pub fn stop(&self) {
        self.backend.stop();
        if let Ok(mut p) = self.prerolled.lock() {
            *p = None;
        }
        let mut st = self.state.write().unwrap_or_else(|e| e.into_inner());
        st.playing = false;
        st.paused = false;
        st.slot = None;
        st.preloaded = false;
        st.position = 0.0;
        st.duration = 0.0;
        st.audio = None;
    }

    pub fn seek(&self, secs: f64) {
        if let Some(p) = self.backend.seek(secs) {
            let mut st = self.state.write().unwrap_or_else(|e| e.into_inner());
            st.position = p;
        }
    }

    pub fn seek_by(&self, delta: f64) {
        let cur = self.backend.position();
        let dur = self.backend.duration();
        let target = (cur + delta).clamp(0.0, if dur > 0.0 { dur } else { f64::MAX });
        self.seek(target);
    }

    pub fn set_volume(&self, v: f32) {
        let v = v.clamp(0.0, 1.5);
        self.backend.set_volume(v);
        self.state.write().unwrap_or_else(|e| e.into_inner()).volume = v;
    }

    pub fn set_muted(&self, m: bool) {
        self.backend.set_muted(m);
        self.state.write().unwrap_or_else(|e| e.into_inner()).muted = m;
    }

    pub fn set_speed(&self, s: f32) {
        self.backend.set_speed(s);
        self.state.write().unwrap_or_else(|e| e.into_inner()).speed = s;
    }

    pub fn set_replaygain(&self, mode: &str) {
        self.backend.set_replaygain(mode);
    }

    pub fn set_audio_device(&self, id: &str) {
        self.backend.set_audio_device(id);
    }

    pub fn sync_prefs(&self, volume: f32, muted: bool, speed: f32, replaygain: &str, exclusive: bool) {
        self.backend.set_volume(volume);
        self.backend.set_muted(muted);
        self.backend.set_speed(speed);
        self.backend.set_replaygain(replaygain);
        self.backend.set_exclusive(exclusive);
    }

    pub fn position(&self) -> f64 {
        self.backend.position()
    }
    pub fn duration(&self) -> f64 {
        self.backend.duration()
    }
}

/// Crea el mejor backend disponible, cableado al canal de eventos.
pub fn create_backend(
    tx: tokio::sync::mpsc::UnboundedSender<PlayerEvent>,
) -> (Arc<dyn Backend>, String) {
    #[cfg(mpv)]
    {
        match crate::player::mpv::MpvBackend::with_sender(tx.clone()) {
            Ok(b) => {
                let name = b.name().to_string();
                tracing::info!("motor de audio: {name} (gapless + FLAC Hi-Res + Atmos)");
                return (Arc::new(b), name);
            }
            Err(e) => tracing::warn!("libmpv no disponible ({e}); usando backend Rust"),
        }
    }
    #[allow(unused_variables)]
    let tx = tx;
    let b = crate::player::fallback::RodioBackend::new(tx);
    let name = b.name().to_string();
    tracing::info!("motor de audio: {name}");
    (Arc::new(b), name)
}

/// Utilidad: formatea "quedan X" para la UI.
pub fn remaining(position: f64, duration: f64) -> String {
    if duration <= 0.0 {
        return String::new();
    }
    let left = (duration - position).max(0.0);
    format!("-{}", crate::util::fmt_duration(left))
}

/// Sincroniza posición/duración desde el backend hacia el estado (por frame).
pub fn sync_state(player: &Player) {
    let pos = player.backend.position();
    let dur = player.backend.duration();
    let paused = player.backend.is_paused();
    if let Ok(mut st) = player.state.write() {
        if (st.position - pos).abs() > 0.01 {
            st.position = pos;
        }
        if dur > 0.0 && (st.duration - dur).abs() > 0.05 {
            st.duration = dur;
        }
        st.paused = paused;
        st.playing = !paused && st.slot.is_some();
        if let Some(a) = player.backend.audio_info() {
            if st.audio.as_ref() != Some(&a) {
                st.audio = Some(a);
            }
        }
    }
}

/// Espera corta usada por el backend de rodio para no quemar CPU.
pub fn sleep(d: Duration) {
    std::thread::sleep(d);
}
