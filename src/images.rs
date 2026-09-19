//! Carga de portadas por HTTP con caché en memoria + disco.
//!
//! Se instala como `egui::ImageLoader`, así que alcanza con
//! `egui::Image::from_uri(url)` en cualquier parte de la UI.
//!
//! Decisiones de rendimiento:
//!   * la red y el decode corren en el runtime de Tokio, nunca en el hilo de UI,
//!   * las imágenes se **reescalan** a un máximo de `MAX_EDGE` px antes de
//!     convertirse en `ColorImage` (una portada de 1500×1500 no va a la GPU),
//!   * los bytes crudos quedan en el caché a disco -> el segundo arranque no
//!     vuelve a bajar nada,
//!   * un LRU acota la memoria,
//!   * los fallos se registran para poder mostrarlos en la UI (una portada que
//!     no carga no debe parecer "la app no muestra imágenes").

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use egui::load::{ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint};
use egui::{ColorImage, Context};

use crate::http::{get_bytes, DiskCache};

/// Lado máximo de la textura decodificada.
const MAX_EDGE: u32 = 768;
/// TTL del caché a disco de imágenes.
const IMG_DISK_TTL: Duration = Duration::from_secs(30 * 24 * 3600);

enum State {
    Loading,
    Ready(Arc<ColorImage>),
    Failed(String),
}

#[derive(Default)]
struct Stats {
    decoded: AtomicU64,
    disk_hits: AtomicU64,
    errors: AtomicU64,
    bytes: AtomicU64,
    /// últimos fallos (url -> motivo), para mostrarlos en la UI
    failures: RwLock<std::collections::VecDeque<(String, String)>>,
}

impl Stats {
    fn record_failure(&self, url: &str, err: &str) {
        if let Ok(mut d) = self.failures.write() {
            d.retain(|(u, _)| u != url);
            d.push_back((url.to_string(), err.to_string()));
            while d.len() > 40 {
                d.pop_front();
            }
        }
    }
}

pub struct NetImageLoader {
    id: String,
    rt: tokio::runtime::Handle,
    disk: Arc<DiskCache>,
    entries: Arc<RwLock<LruCache<String, Arc<Mutex<State>>>>>,
    inflight: Arc<Mutex<std::collections::HashSet<String>>>,
    stats: Arc<Stats>,
}

impl NetImageLoader {
    pub fn new(rt: tokio::runtime::Handle, disk: Arc<DiskCache>) -> Self {
        Self {
            id: "sonido::NetImageLoader".to_owned(),
            rt,
            disk,
            entries: Arc::new(RwLock::new(LruCache::new(2048))),
            inflight: Arc::new(Mutex::new(Default::default())),
            stats: Arc::new(Stats::default()),
        }
    }

    /// (decodificadas, hits de disco, errores, bytes, entradas en caché)
    pub fn stats(&self) -> (u64, u64, u64, u64, usize) {
        (
            self.stats.decoded.load(Ordering::Relaxed),
            self.stats.disk_hits.load(Ordering::Relaxed),
            self.stats.errors.load(Ordering::Relaxed),
            self.stats.bytes.load(Ordering::Relaxed),
            self.entries.read().map(|m| m.len()).unwrap_or(0),
        )
    }

    /// Últimos errores de carga (para el panel de depuración).
    pub fn failures(&self) -> Vec<(String, String)> {
        self.stats
            .failures
            .read()
            .map(|d| d.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Olvida todo: caché propio + texturas de egui.
    pub fn forget_all_egui(&self, ctx: &Context) {
        ctx.forget_all_images();
        if let Ok(mut m) = self.entries.write() {
            m.clear();
        }
    }

    fn entry(&self, uri: &str) -> Arc<Mutex<State>> {
        let key = uri.to_string();
        if let Some(e) = self.entries.read().ok().and_then(|m| m.peek(&key).cloned()) {
            return e;
        }
        let e = Arc::new(Mutex::new(State::Loading));
        if let Ok(mut m) = self.entries.write() {
            m.put(key, e.clone());
        }
        e
    }

    fn spawn(&self, ctx: Context, uri: String) {
        {
            let mut f = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
            if !f.insert(uri.clone()) {
                return;
            }
        }
        let disk = self.disk.clone();
        let stats = self.stats.clone();
        let stats2 = self.stats.clone();
        let entries = self.entries.clone();
        let inflight = self.inflight.clone();
        let uri2 = uri.clone();

        self.rt.spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                let key = format!("img|{uri2}");
                if let Some((bytes, _age)) = disk.get(&key, IMG_DISK_TTL) {
                    stats.disk_hits.fetch_add(1, Ordering::Relaxed);
                    return decode(bytes, &stats);
                }
                match get_bytes(&uri2, &[], Duration::from_secs(20)) {
                    Ok(bytes) => {
                        disk.put(&key, &bytes);
                        decode(bytes, &stats)
                    }
                    Err(e) => Err(format!("{e}")),
                }
            })
            .await;

            let state = match res {
                Ok(Ok(img)) => State::Ready(Arc::new(img)),
                Ok(Err(e)) => {
                    stats2.errors.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!("portada falló: {uri} → {e}");
                    stats2.record_failure(&uri, &e);
                    State::Failed(e)
                }
                Err(e) => {
                    stats2.errors.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!("portada falló (join): {uri} → {e}");
                    stats2.record_failure(&uri, &e.to_string());
                    State::Failed(e.to_string())
                }
            };
            if let Ok(mut m) = entries.write() {
                if let Some(slot) = m.get_mut(&uri) {
                    if let Ok(mut s) = slot.lock() {
                        *s = state;
                    }
                }
            }
            if let Ok(mut f) = inflight.lock() {
                f.remove(&uri);
            }
            ctx.request_repaint();
        });
    }
}

fn decode(bytes: Vec<u8>, stats: &Stats) -> Result<ColorImage, String> {
    stats.bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
    let img = image::load_from_memory(&bytes).map_err(|e| format!("decode: {e}"))?;
    let longest = img.width().max(img.height());
    let img = if longest > MAX_EDGE {
        let scale = MAX_EDGE as f32 / longest as f32;
        let w = ((img.width() as f32 * scale).round().max(1.0)) as u32;
        let h = ((img.height() as f32 * scale).round().max(1.0)) as u32;
        img.resize_exact(w, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    stats.decoded.fetch_add(1, Ordering::Relaxed);
    Ok(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

impl ImageLoader for NetImageLoader {
    fn id(&self) -> &str {
        &self.id
    }

    fn load(&self, ctx: &Context, uri: &str, _size_hint: SizeHint) -> ImageLoadResult {
        if !is_http(uri) {
            return Err(LoadError::NotSupported);
        }
        let entry = self.entry(uri);
        let guard = entry.lock().unwrap_or_else(|e| e.into_inner());
        match &*guard {
            State::Loading => {
                drop(guard);
                self.spawn(ctx.clone(), uri.to_string());
                Ok(ImagePoll::Pending { size: None })
            }
            State::Ready(img) => Ok(ImagePoll::Ready { image: img.clone() }),
            State::Failed(e) => Err(LoadError::Loading(e.clone())),
        }
    }

    fn forget(&self, uri: &str) {
        if let Ok(mut m) = self.entries.write() {
            m.pop(&uri.to_string());
        }
    }

    fn forget_all(&self) {
        if let Ok(mut m) = self.entries.write() {
            m.clear();
        }
    }

    fn byte_size(&self) -> usize {
        self.stats.bytes.load(Ordering::Relaxed) as usize
    }

    fn has_pending(&self) -> bool {
        self.inflight
            .lock()
            .map(|f| !f.is_empty())
            .unwrap_or(false)
    }
}

fn is_http(uri: &str) -> bool {
    uri.starts_with("http://") || uri.starts_with("https://")
}

// ─────────────────────────────── LRU minimalista ───────────────────────────────

pub struct LruCache<K, V> {
    map: std::collections::HashMap<K, (V, u64)>,
    counter: u64,
    cap: usize,
}

impl<K: std::hash::Hash + Eq + Clone, V> LruCache<K, V> {
    pub fn new(cap: usize) -> Self {
        Self {
            map: std::collections::HashMap::with_capacity(256),
            counter: 0,
            cap,
        }
    }

    pub fn peek(&self, k: &K) -> Option<&V> {
        self.map.get(k).map(|(v, _)| v)
    }

    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        self.counter += 1;
        let c = self.counter;
        self.map.get_mut(k).map(move |e| {
            e.1 = c;
            &mut e.0
        })
    }

    pub fn put(&mut self, k: K, v: V) {
        self.counter += 1;
        let c = self.counter;
        self.map.insert(k, (v, c));
        if self.map.len() > self.cap {
            self.evict_oldest(self.map.len() - self.cap);
        }
    }

    pub fn pop(&mut self, k: &K) -> Option<V> {
        self.map.remove(k).map(|(v, _)| v)
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn evict_oldest(&mut self, n: usize) {
        let mut keys: Vec<(K, u64)> = self.map.iter().map(|(k, (_, c))| (k.clone(), *c)).collect();
        keys.sort_by_key(|(_, c)| *c);
        for (k, _) in keys.into_iter().take(n) {
            self.map.remove(&k);
        }
    }
}
