//! Configuración persistida + rutas XDG.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::models::{LibraryTrack, LocalPlaylist};

pub use crate::models::{LibraryTrack as LibTrack, LocalPlaylist as LibPlaylist};
use crate::net::NetworkKind;

pub const APP_ID: &str = "sonido";
pub const APP_NAME: &str = "Sonido";

fn xdg(env: &str, fallback: &str) -> PathBuf {
    if let Ok(v) = std::env::var(env) {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
        .join(fallback)
        .join(APP_ID)
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
}
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

// ─────────────────────────── addons instalados ───────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonEntry {
    /// URL base normalizada (sin `/manifest.json`).
    pub base: String,
    #[serde(default)]
    pub name_override: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Prioridad para la cadena de playback (menor = primero).
    #[serde(default)]
    pub priority: u32,
    /// Valores de settings declarados por el addon.
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    /// Valores alternativos cuando la red es celular/metered (perNetwork).
    #[serde(default)]
    pub settings_cellular: BTreeMap<String, String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub installed_at: u64,
    /// Último manifest conocido, cacheado para arrancar la UI sin red.
    #[serde(default)]
    pub manifest_cache: Option<crate::models::Manifest>,
    #[serde(default)]
    pub manifest_at: u64,
    #[serde(default)]
    pub note: Option<String>,
}

fn default_true() -> bool {
    true
}

impl AddonEntry {
    pub fn manifest_url(&self) -> String {
        crate::util::join_url(&self.base, "manifest.json")
    }

    /// Parámetros de settings según la red actual.
    pub fn settings_for(&self, net: NetworkKind) -> &BTreeMap<String, String> {
        match net {
            NetworkKind::Cellular if !self.settings_cellular.is_empty() => &self.settings_cellular,
            _ => &self.settings,
        }
    }

    pub fn header_list(&self) -> Vec<(String, String)> {
        self.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

// ─────────────────────────── preferencias de la app ───────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    #[serde(default = "d_volume")]
    pub volume: f32,
    #[serde(default)]
    pub muted: bool,
    #[serde(default = "d_speed")]
    pub speed: f32,
    #[serde(default)]
    pub shuffle: bool,
    /// "off" | "all" | "one"
    #[serde(default = "d_repeat")]
    pub repeat: String,
    #[serde(default = "d_true")]
    pub gapless: bool,
    /// Segundos de anticipación con los que se resuelve el siguiente tema.
    #[serde(default = "d_preroll")]
    pub preroll_secs: f32,
    /// "off" | "track" | "album"
    #[serde(default = "d_off")]
    pub replaygain: String,
    #[serde(default)]
    pub audio_device: Option<String>,
    #[serde(default = "d_true")]
    pub audio_exclusive: bool,
    /// Preferencias para rankear streams.
    #[serde(default = "d_true")]
    pub prefer_hires: bool,
    #[serde(default)]
    pub prefer_atmos: bool,
    #[serde(default = "d_true")]
    pub hide_explicit: bool,
    /// Modo de búsqueda: todos los addons a la vez, o uno solo.
    #[serde(default)]
    pub search_all_addons: bool,
    #[serde(default)]
    pub active_addon: Option<String>,
    #[serde(default = "d_true")]
    pub crossfade_prefetch: bool,
    /// Verificar la URL de audio (HEAD/Range) antes de mandarla al motor.
    /// Cuesta un round-trip chico pero convierte "no suena" en un error concreto.
    #[serde(default = "d_true")]
    pub probe_stream: bool,
    /// Rellenar portadas faltantes por ISRC usando la API pública de iTunes.
    /// Solo se consulta lo que el addon NO mandó; el resultado se cachea 7 días.
    #[serde(default = "d_true")]
    pub artwork_lookup: bool,
    #[serde(default = "d_cache_mb")]
    pub cache_mb: u64,
    #[serde(default = "d_manifest_ttl")]
    pub manifest_ttl_secs: u64,
    #[serde(default = "d_meta_ttl")]
    pub metadata_ttl_secs: u64,
    #[serde(default = "d_true")]
    pub show_fps: bool,
    #[serde(default = "d_theme")]
    pub theme: String,
    #[serde(default = "d_true")]
    pub normalize_volume: bool,
}

fn d_volume() -> f32 { 0.85 }
fn d_speed() -> f32 { 1.0 }
fn d_repeat() -> String { "off".into() }
fn d_preroll() -> f32 { 12.0 }
fn d_off() -> String { "off".into() }
fn d_cache_mb() -> u64 { 512 }
fn d_manifest_ttl() -> u64 { 3600 * 6 }
fn d_meta_ttl() -> u64 { 3600 * 24 * 3 }
fn d_theme() -> String { "eclipse".into() }
fn d_true() -> bool { true }

impl Default for Prefs {
    fn default() -> Self {
        Self {
            volume: d_volume(),
            muted: false,
            speed: d_speed(),
            shuffle: false,
            repeat: d_repeat(),
            gapless: d_true(),
            preroll_secs: d_preroll(),
            replaygain: d_off(),
            audio_device: None,
            audio_exclusive: d_true(),
            prefer_hires: d_true(),
            prefer_atmos: false,
            hide_explicit: d_true(),
            search_all_addons: false,
            active_addon: None,
            crossfade_prefetch: d_true(),
            probe_stream: d_true(),
            artwork_lookup: d_true(),
            cache_mb: d_cache_mb(),
            manifest_ttl_secs: d_manifest_ttl(),
            metadata_ttl_secs: d_meta_ttl(),
            show_fps: d_true(),
            theme: d_theme(),
            normalize_volume: d_true(),
        }
    }
}

// ─────────────────────────────── biblioteca ───────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    #[serde(default)]
    pub favorites: Vec<LibraryTrack>,
    #[serde(default)]
    pub playlists: Vec<LocalPlaylist>,
    /// Historial de reproducción (clave -> contador + último visto).
    #[serde(default)]
    pub play_counts: BTreeMap<String, PlayCount>,
    #[serde(default)]
    pub recent: Vec<LibraryTrack>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayCount {
    pub plays: u32,
    pub last_played: u64,
    pub title: String,
    pub artist: String,
}

impl Library {
    pub fn is_favorite(&self, addon_id: &str, track_id: &str) -> bool {
        self.favorites
            .iter()
            .any(|t| t.addon_id == addon_id && t.track_id == track_id)
    }

    pub fn toggle_favorite(&mut self, t: LibraryTrack) -> bool {
        if let Some(i) = self
            .favorites
            .iter()
            .position(|x| x.addon_id == t.addon_id && x.track_id == t.track_id)
        {
            self.favorites.remove(i);
            false
        } else {
            self.favorites.insert(0, t);
            true
        }
    }

    pub fn record_play(&mut self, t: &LibraryTrack) {
        let key = t.key();
        let e = self.play_counts.entry(key).or_default();
        e.plays += 1;
        e.last_played = now_secs();
        e.title = t.title.clone();
        e.artist = t.artist.clone();
        // recientes (máx 200)
        self.recent.retain(|x| x.key() != t.key());
        self.recent.insert(0, t.clone());
        self.recent.truncate(200);
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ─────────────────────────────── store ───────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFile {
    #[serde(default)]
    pub addons: Vec<AddonEntry>,
    #[serde(default)]
    pub prefs: Prefs,
}

/// Guarda la configuración y la biblioteca en disco, con escritura atómica.
#[derive(Debug)]
pub struct Store {
    config_path: PathBuf,
    library_path: PathBuf,
    pub config: std::sync::RwLock<ConfigFile>,
    pub library: std::sync::RwLock<Library>,
    dirty_config: AtomicBool,
    dirty_library: AtomicBool,
    last_save: AtomicU64,
}

impl Store {
    pub fn load() -> Self {
        let cd = config_dir();
        let dd = data_dir();
        let _ = std::fs::create_dir_all(&cd);
        let _ = std::fs::create_dir_all(&dd);
        let config_path = cd.join("addons.json");
        let library_path = dd.join("library.json");

        let config = read_json::<ConfigFile>(&config_path).unwrap_or_default();
        let library = read_json::<Library>(&library_path).unwrap_or_default();

        Self {
            config_path,
            library_path,
            config: std::sync::RwLock::new(config),
            library: std::sync::RwLock::new(library),
            dirty_config: AtomicBool::new(false),
            dirty_library: AtomicBool::new(false),
            last_save: AtomicU64::new(0),
        }
    }

    pub fn mark_config_dirty(&self) {
        self.dirty_config.store(true, Ordering::Relaxed);
    }
    pub fn mark_library_dirty(&self) {
        self.dirty_library.store(true, Ordering::Relaxed);
    }

    /// Guarda si hay cambios (o si pasaron > 5 s desde el último flush).
    pub fn flush_if_needed(&self, force: bool) {
        let now = now_secs();
        let last = self.last_save.load(Ordering::Relaxed);
        let dirty = self.dirty_config.load(Ordering::Relaxed)
            || self.dirty_library.load(Ordering::Relaxed);
        if !dirty && !force {
            return;
        }
        if !force && now.saturating_sub(last) < 3 {
            return;
        }
        self.save();
    }

    pub fn save(&self) {
        if self.dirty_config.swap(false, Ordering::Relaxed) {
            if let Ok(c) = self.config.read() {
                write_json(&self.config_path, &*c);
            }
        }
        if self.dirty_library.swap(false, Ordering::Relaxed) {
            if let Ok(l) = self.library.read() {
                write_json(&self.library_path, &*l);
            }
        }
        self.last_save.store(now_secs(), Ordering::Relaxed);
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }
    pub fn library_path(&self) -> &PathBuf {
        &self.library_path
    }
}

fn read_json<T: serde::de::DeserializeOwned + Default>(p: &PathBuf) -> Option<T> {
    let s = std::fs::read_to_string(p).ok()?;
    match serde_json::from_str::<T>(&s) {
        Ok(v) => Some(v),
        Err(e) => {
            // respaldo del archivo corrupto para no perder datos
            let _ = std::fs::rename(p, p.with_extension("corrupt"));
            tracing::error!("no se pudo leer {}: {e}", p.display());
            Some(T::default())
        }
    }
}

fn write_json<T: Serialize>(p: &PathBuf, v: &T) {
    let tmp = p.with_extension("tmp");
    match serde_json::to_vec_pretty(v) {
        Ok(bytes) => {
            if std::fs::write(&tmp, &bytes).is_ok() {
                let _ = std::fs::rename(&tmp, p);
            }
        }
        Err(e) => tracing::error!("serializando {}: {e}", p.display()),
    }
}
