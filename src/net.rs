//! Red: tipo de conexión, registro de addons instalados y fan-out de peticiones.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{anyhow, Result};

use crate::addon::{Addon, AddonApi, ManifestCache};
use crate::config::{AddonEntry, Store};
use crate::http::Http;
use crate::models::*;
use crate::resolve::ResolvedStream;

// ─────────────────────────────── tipo de red ───────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkKind {
    Wifi,
    Cellular,
}

impl NetworkKind {
    pub fn detect() -> Self {
        if crate::http::is_metered() {
            NetworkKind::Cellular
        } else {
            NetworkKind::Wifi
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            NetworkKind::Wifi => "Wi-Fi",
            NetworkKind::Cellular => "Celular",
        }
    }
}

fn lock<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}
fn lock_mut<T>(l: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(|e| e.into_inner())
}

// ──────────────────────────────── registro ────────────────────────────────

/// Todos los addons instalados + el store persistido.
pub struct Registry {
    pub store: Arc<Store>,
    pub api: AddonApi,
    addons: RwLock<Vec<Addon>>,
    manifests: ManifestCache,
    refresh_busy: AtomicBool,
}

impl Registry {
    pub fn new(store: Arc<Store>, http: Http) -> Self {
        let mut addons = Vec::new();
        let manifests = ManifestCache::default();
        {
            let cfg = lock(&store.config);
            for e in &cfg.addons {
                let m = e.manifest_cache.clone().unwrap_or_else(|| Manifest {
                    id: e.base.clone(),
                    name: e.name_override.clone().unwrap_or_else(|| e.base.clone()),
                    ..Default::default()
                });
                manifests.put(&e.base, m.clone());
                addons.push(Addon {
                    base: e.base.clone(),
                    manifest: Arc::new(RwLock::new(m)),
                    entry: Arc::new(RwLock::new(e.clone())),
                });
            }
        }
        Self {
            store,
            api: AddonApi::new(http),
            addons: RwLock::new(addons),
            manifests,
            refresh_busy: AtomicBool::new(false),
        }
    }

    pub fn http(&self) -> &Http {
        &self.api.http
    }

    pub fn all(&self) -> Vec<Addon> {
        lock(&self.addons).clone()
    }

    pub fn enabled(&self) -> Vec<Addon> {
        let mut v: Vec<Addon> = lock(&self.addons).iter().filter(|a| a.enabled()).cloned().collect();
        v.sort_by_key(|a| (a.priority(), a.name()));
        v
    }

    /// Cadena de reproducción: addons habilitados ordenados por prioridad.
    pub fn playback_chain(&self) -> Vec<Addon> {
        self.enabled()
            .into_iter()
            .filter(|a| crate::addon::rg(&a.manifest).can_stream())
            .collect()
    }

    pub fn by_id(&self, id: &str) -> Option<Addon> {
        lock(&self.addons).iter().find(|a| a.id() == id).cloned()
    }

    pub fn by_base(&self, base: &str) -> Option<Addon> {
        lock(&self.addons)
            .iter()
            .find(|a| a.base == base)
            .cloned()
    }

    pub fn count(&self) -> usize {
        lock(&self.addons).len()
    }

    /// Instala (o reinstala) un addon desde su URL.
    pub fn install(&self, url: &str, force_manifest: bool) -> Result<Addon> {
        let base = crate::util::normalize_addon_base(url)?;
        let manifest = match self.api.fetch_manifest(&base, Duration::ZERO) {
            Ok(m) => {
                self.manifests.put(&base, m.clone());
                m
            }
            Err(e) => {
                // ¿ya lo teníamos instalado? -> reusamos el manifest cacheado
                let cached = lock(&self.store.config)
                    .addons
                    .iter()
                    .find(|a| a.base == base)
                    .and_then(|a| a.manifest_cache.clone());
                match cached {
                    Some(m) if !force_manifest => {
                        tracing::warn!("manifest fresco falló ({e}); usando el cacheado");
                        m
                    }
                    _ => return Err(e),
                }
            }
        };

        let now = crate::config::now_secs();
        let mut cfg = lock_mut(&self.store.config);
        let existing = cfg.addons.iter_mut().find(|a| a.base == base);
        let entry = match existing {
            Some(e) => {
                e.manifest_cache = Some(manifest.clone());
                e.manifest_at = now;
                // limpiar settings que el addon ya no declara
                let valid: Vec<String> = manifest.settings.iter().map(|s| s.key.clone()).collect();
                e.settings.retain(|k, _| valid.contains(k));
                e.settings_cellular.retain(|k, _| valid.contains(k));
                e.clone()
            }
            None => {
                let priority = cfg.addons.len() as u32;
                let e = AddonEntry {
                    base: base.clone(),
                    name_override: None,
                    enabled: true,
                    priority,
                    settings: BTreeMap::new(),
                    settings_cellular: BTreeMap::new(),
                    headers: BTreeMap::new(),
                    installed_at: now,
                    manifest_cache: Some(manifest.clone()),
                    manifest_at: now,
                    note: None,
                };
                cfg.addons.push(e.clone());
                e
            }
        };
        drop(cfg);
        self.store.mark_config_dirty();

        let addon = Addon {
            base: base.clone(),
            manifest: Arc::new(RwLock::new(manifest)),
            entry: Arc::new(RwLock::new(entry)),
        };

        let mut list = lock_mut(&self.addons);
        if let Some(pos) = list.iter().position(|a| a.base == base) {
            list[pos] = addon.clone();
        } else {
            list.push(addon.clone());
        }
        Ok(addon)
    }

    pub fn remove(&self, base: &str) {
        lock_mut(&self.addons).retain(|a| a.base != base);
        self.manifests.forget(base);
        {
            let mut cfg = lock_mut(&self.store.config);
            cfg.addons.retain(|a| a.base != base);
            for (i, a) in cfg.addons.iter_mut().enumerate() {
                a.priority = i as u32;
            }
        }
        self.store.mark_config_dirty();
    }

    pub fn set_enabled(&self, base: &str, on: bool) {
        self.mutate(base, |e| e.enabled = on);
    }

    pub fn set_name(&self, base: &str, name: String) {
        self.mutate(base, |e| {
            e.name_override = if name.trim().is_empty() { None } else { Some(name) }
        });
        self.sync_addon(base);
    }

    pub fn set_setting(&self, base: &str, key: &str, value: String, net: NetworkKind) {
        self.mutate(base, |e| {
            let map = match net {
                NetworkKind::Cellular => &mut e.settings_cellular,
                NetworkKind::Wifi => &mut e.settings,
            };
            if value.is_empty() {
                map.remove(key);
            } else {
                map.insert(key.to_string(), value);
            }
            // el mapa de Wi-Fi es el "base"; el de celular solo pisa si está definido
            if matches!(net, NetworkKind::Wifi) {
                e.settings_cellular.remove(key);
            }
        });
        self.sync_addon(base);
    }

    pub fn set_header(&self, base: &str, key: &str, value: String) {
        self.mutate(base, |e| {
            if value.is_empty() {
                e.headers.remove(key);
            } else {
                e.headers.insert(key.to_string(), value);
            }
        });
        self.sync_addon(base);
    }

    pub fn set_priority(&self, base: &str, p: u32) {
        self.mutate(base, |e| e.priority = p);
        self.sync_addon(base);
    }

    /// Reordena la lista (drag & drop) y renumera prioridades.
    pub fn reorder(&self, order: Vec<String>) {
        {
            let mut cfg = lock_mut(&self.store.config);
            let mut idx: BTreeMap<String, usize> = BTreeMap::new();
            for (i, b) in order.iter().enumerate() {
                idx.insert(b.clone(), i);
            }
            for a in cfg.addons.iter_mut() {
                if let Some(i) = idx.get(&a.base) {
                    a.priority = *i as u32;
                }
            }
            cfg.addons.sort_by_key(|a| a.priority);
        }
        self.store.mark_config_dirty();
        let mut list = lock_mut(&self.addons);
        let mut idx: BTreeMap<String, usize> = BTreeMap::new();
        for (i, b) in order.iter().enumerate() {
            idx.insert(b.clone(), i);
        }
        for a in list.iter() {
            if let Some(i) = idx.get(&a.base) {
                let p = *i as u32;
                crate::addon::wg(&a.entry).priority = p;
            }
        }
        list.sort_by_key(|a| a.priority());
    }

    fn mutate(&self, base: &str, f: impl FnOnce(&mut AddonEntry)) {
        {
            let mut cfg = lock_mut(&self.store.config);
            if let Some(e) = cfg.addons.iter_mut().find(|a| a.base == base) {
                f(e);
            }
        }
        self.store.mark_config_dirty();
    }

    fn sync_addon(&self, base: &str) {
        let entry = lock(&self.store.config)
            .addons
            .iter()
            .find(|a| a.base == base)
            .cloned();
        if let Some(e) = entry {
            let list = lock(&self.addons);
            if let Some(a) = list.iter().find(|a| a.base == base) {
                *a.entry.write().unwrap_or_else(|p| p.into_inner()) = e;
            }
        }
    }

    /// Refresca los manifests (en background) y devuelve cuántos cambiaron.
    pub fn refresh_manifests(&self, _net: NetworkKind) -> Result<usize> {
        if self.refresh_busy.swap(true, Ordering::AcqRel) {
            return Ok(0);
        }
        let guard = BusyGuard(&self.refresh_busy);
        let bases: Vec<String> = lock(&self.addons).iter().map(|a| a.base.clone()).collect();
        let mut changed = 0;
        for base in bases {
            let ttl = Duration::from_secs(6 * 3600);
            match self.api.fetch_manifest(&base, ttl) {
                Ok(m) => {
                    let old = lock(&self.addons)
                        .iter()
                        .find(|a| a.base == base)
                        .map(|a| crate::addon::rg(&a.manifest).clone());
                    let differs = old.as_ref().map(|o| !same_manifest(o, &m)).unwrap_or(true);
                    if differs {
                        changed += 1;
                        let list = lock(&self.addons);
                        if let Some(a) = list.iter().find(|a| a.base == base) {
                            *a.manifest.write().unwrap_or_else(|p| p.into_inner()) = m.clone();
                        }
                        self.manifests.put(&base, m.clone());
                        let mut cfg = lock_mut(&self.store.config);
                        if let Some(e) = cfg.addons.iter_mut().find(|a| a.base == base) {
                            e.manifest_cache = Some(m.clone());
                            e.manifest_at = crate::config::now_secs();
                            let valid: Vec<String> = m.settings.iter().map(|s| s.key.clone()).collect();
                            e.settings.retain(|k, _| valid.contains(k));
                            e.settings_cellular.retain(|k, _| valid.contains(k));
                        }
                        drop(cfg);
                        self.store.mark_config_dirty();
                    }
                }
                Err(e) => {
                    tracing::warn!("refresh manifest {base}: {e}");
                }
            }
        }
        drop(guard);
        Ok(changed)
    }

    pub fn is_refreshing(&self) -> bool {
        self.refresh_busy.load(Ordering::Relaxed)
    }

    /// Al cambiar un setting no hace falta invalidar: la clave del caché a disco
    /// incluye los query params, así que un valor nuevo genera otra entrada.
    pub fn on_settings_changed(&self, _base: &str) {}

    pub fn cached_manifest(&self, base: &str) -> Option<Manifest> {
        self.manifests.get(base)
    }

    pub fn clear_disk_cache(&self) {
        self.api.http.cache.clear();
    }
}

/// Restaura el flag de "refresh en curso" al salir del scope.
struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn same_manifest(a: &Manifest, b: &Manifest) -> bool {
    a.version == b.version && a.settings.len() == b.settings.len() && a.resources == b.resources
}

// ──────────────────────────── peticiones compuestas ────────────────────────────

/// Resultado de una petición a un addon, para el fan-out.
#[derive(Debug, Clone)]
pub enum AddonResult {
    Search {
        addon_id: String,
        addon_name: String,
        res: SearchResponse,
        elapsed: Duration,
    },
    SearchErr {
        addon_id: String,
        addon_name: String,
        error: String,
        elapsed: Duration,
    },
    Catalog {
        addon_id: String,
        addon_name: String,
        catalog_id: String,
        catalog_name: String,
        kind: ItemKind,
        items: Vec<CatalogItem>,
        end_of_row: bool,
        skip: u32,
        elapsed: Duration,
    },
    CatalogErr {
        addon_id: String,
        catalog_id: String,
        error: String,
    },
    Album {
        addon_id: String,
        album: Album,
        elapsed: Duration,
    },
    Artist {
        addon_id: String,
        artist: Artist,
        elapsed: Duration,
    },
    Playlist {
        addon_id: String,
        playlist: Playlist,
        elapsed: Duration,
    },
    DetailErr {
        addon_id: String,
        kind: String,
        id: String,
        error: String,
    },
    Stream {
        addon_id: String,
        resolved: ResolvedStream,
        elapsed: Duration,
    },
    StreamErr {
        addon_id: String,
        track_id: String,
        error: String,
    },
    /// Portadas conseguidas por ISRC para temas que venían sin artwork.
    Artwork {
        map: BTreeMap<String, String>,
    },
    ManifestChanged,
    Installed {
        addon_id: String,
        name: String,
    },
    InstallErr {
        url: String,
        error: String,
    },
    Toast {
        msg: String,
        error: bool,
    },
}

/// Versión sincrónica: se llama desde un worker bloqueante.
pub fn spawn_catalog_blocking(
    reg: Arc<Registry>,
    addon: Addon,
    def: CatalogDef,
    skip: u32,
    net: NetworkKind,
    tx: tokio::sync::mpsc::UnboundedSender<AddonResult>,
) {
    {
        let t = std::time::Instant::now();
        let id = addon.id();
        let name = addon.name();
        let ttl = Duration::from_secs(30 * 60);
        match reg.api.catalog(&addon, &def.id, skip, net, ttl) {
            Ok(res) => {
                let end = res.items.len() < 100;
                let _ = tx.send(AddonResult::Catalog {
                    addon_id: id,
                    addon_name: name,
                    catalog_id: def.id.clone(),
                    catalog_name: def.name.clone(),
                    kind: ItemKind::parse(&def.r#type),
                    items: res.items,
                    end_of_row: end,
                    skip,
                    elapsed: t.elapsed(),
                });
            }
            Err(e) => {
                let _ = tx.send(AddonResult::CatalogErr {
                    addon_id: id,
                    catalog_id: def.id.clone(),
                    error: friendly_err(&e),
                });
            }
        }
    }
}

/// Versión sincrónica: se llama desde un worker bloqueante.
pub fn spawn_detail_blocking(
    reg: Arc<Registry>,
    addon: Addon,
    kind: ItemKind,
    id: String,
    net: NetworkKind,
    tx: tokio::sync::mpsc::UnboundedSender<AddonResult>,
) {
    {
        let t = std::time::Instant::now();
        let aid = addon.id();
        let ttl = Duration::from_secs(3 * 24 * 3600);
        let r = match kind {
            ItemKind::Album => reg
                .api
                .album(&addon, &id, net, ttl)
                .map(|a| AddonResult::Album {
                    addon_id: aid.clone(),
                    album: a,
                    elapsed: t.elapsed(),
                }),
            ItemKind::Artist => reg
                .api
                .artist(&addon, &id, net, ttl)
                .map(|a| AddonResult::Artist {
                    addon_id: aid.clone(),
                    artist: a,
                    elapsed: t.elapsed(),
                }),
            ItemKind::Playlist => reg
                .api
                .playlist(&addon, &id, net, ttl)
                .map(|p| AddonResult::Playlist {
                    addon_id: aid.clone(),
                    playlist: p,
                    elapsed: t.elapsed(),
                }),
            _ => Err(anyhow!("tipo no soportado")),
        };
        match r {
            Ok(v) => {
                let _ = tx.send(v);
            }
            Err(e) => {
                let _ = tx.send(AddonResult::DetailErr {
                    addon_id: aid,
                    kind: kind.as_str().to_string(),
                    id,
                    error: friendly_err(&e),
                });
            }
        }
    }
}

pub fn friendly_err(e: &anyhow::Error) -> String {
    // primero buscamos un HttpError en la cadena (tiene el status real)
    if let Some(he) = e.downcast_ref::<crate::http::HttpError>() {
        return match he.status {
            401 | 403 => format!("{} — token/permiso rechazado por el addon", he.status),
            404 => format!("404 — el addon no expone ese endpoint ({})", short_url(&he.url)),
            429 => "429 — el addon limitó la tasa de pedidos".into(),
            500..=599 => format!("{} — error del servidor del addon", he.status),
            other => format!("HTTP {other} del addon"),
        };
    }
    let s = e.to_string();
    if s.contains("HTTP 403") {
        return "403 — token/permiso rechazado por el addon".into();
    }
    if s.contains("HTTP 404") {
        return "404 — el addon no expone ese endpoint".into();
    }
    if s.contains("HTTP 429") {
        return "429 — el addon limitó la tasa de pedidos".into();
    }
    if s.contains("timeout") {
        return "timeout — el addon tardó demasiado".into();
    }
    if s.contains("no se pudo resolver") {
        return "DNS falló para el addon".into();
    }
    crate::util::ellipsize(&s, 180)
}

fn short_url(u: &str) -> String {
    u.rsplit('/').next().unwrap_or(u).to_string()
}

/// Chequeo rápido de conectividad hacia un host (para el panel de diagnóstico).
pub fn ping(url: &str) -> Result<Duration> {
    let t = std::time::Instant::now();
    let (status, _, _) = crate::http::probe(url, &[])?;
    if status >= 500 {
        return Err(anyhow!("HTTP {status}"));
    }
    Ok(t.elapsed())
}
