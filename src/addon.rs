//! Cliente del protocolo de addons de Eclipse Music.
//!
//! Cada addon instalado es una [`Addon`]: base URL + manifest + settings.
//! Todos los métodos son bloqueantes y pensados para correr en un worker.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;


use anyhow::{anyhow, Result};
use std::sync::RwLock;

use crate::config::AddonEntry;
use crate::http::Http;
use crate::models::*;
use crate::net::NetworkKind;

/// Lectura/escritura de `RwLock` tolerante a envenenamiento: un panic en otro
/// hilo no debe tumbar la app entera.
pub fn rg<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(|e| e.into_inner())
}
pub fn wg<T>(l: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(|e| e.into_inner())
}

/// TTL por defecto del manifest en caché a disco.
pub const MANIFEST_TTL: Duration = Duration::from_secs(6 * 3600);
/// TTL de detalles (álbum/artista/playlist) en caché a disco.
pub const DETAIL_TTL: Duration = Duration::from_secs(3 * 24 * 3600);

#[derive(Clone)]
pub struct Addon {
    pub base: String,
    pub manifest: Arc<RwLock<Manifest>>,
    /// entry viva (settings, enabled, prioridad, headers)
    pub entry: Arc<RwLock<AddonEntry>>,
}

impl Addon {
    pub fn id(&self) -> String {
        let m = rg(&self.manifest);
        if m.id.is_empty() {
            self.base.clone()
        } else {
            m.id.clone()
        }
    }

    pub fn name(&self) -> String {
        let ov = rg(&self.entry).name_override.clone();
        if let Some(n) = ov.filter(|n| !n.trim().is_empty()) {
            return n;
        }
        rg(&self.manifest).display_name()
    }

    pub fn icon(&self) -> Option<String> {
        rg(&self.manifest).icon.clone()
    }

    pub fn version(&self) -> String {
        rg(&self.manifest).version.clone()
    }

    pub fn enabled(&self) -> bool {
        rg(&self.entry).enabled
    }

    pub fn priority(&self) -> u32 {
        rg(&self.entry).priority
    }

    pub fn content_type(&self) -> String {
        rg(&self.manifest)
            .content_type
            .clone()
            .unwrap_or_else(|| "music".into())
    }

    pub fn has(&self, r: &str) -> bool {
        rg(&self.manifest).has(r)
    }

    pub fn catalogs(&self) -> Vec<CatalogDef> {
        rg(&self.manifest).catalogs.clone()
    }

    pub fn settings_defs(&self) -> Vec<SettingDef> {
        rg(&self.manifest).settings.clone()
    }

    /// Query params efectivos = settings declarados (con default si el usuario no
    /// los tocó) según la red actual.
    pub fn settings_params(&self, net: NetworkKind) -> Vec<(String, String)> {
        let entry = rg(&self.entry).clone();
        let man = rg(&self.manifest).clone();
        let user = entry.settings_for(net);
        let mut out = Vec::with_capacity(man.settings.len());
        for def in &man.settings {
            let v = user
                .get(&def.key)
                .cloned()
                .or_else(|| def.default_str())
                .unwrap_or_default();
            if !v.is_empty() {
                out.push((def.key.clone(), v));
            }
        }
        out
    }

    fn headers(&self) -> Vec<(String, String)> {
        rg(&self.entry).header_list()
    }

    fn url(&self, path: &str) -> String {
        crate::util::join_url(&self.base, path)
    }
}

// ──────────────────────────────── API del addon ────────────────────────────────

pub struct AddonApi {
    pub http: Http,
}

impl AddonApi {
    pub fn new(http: Http) -> Self {
        Self { http }
    }

    /// Descarga e interpreta `manifest.json`.
    pub fn fetch_manifest(&self, base: &str, ttl: Duration) -> Result<Manifest> {
        let url = crate::util::join_url(base, "manifest.json");
        let m: Manifest = crate::http::JsonRequest::new(&url)
            .cache(&self.http.cache, ttl)
            .timeout(Duration::from_secs(15))
            .json()
            .map_err(|e| {
                if let Some(he) = e.downcast_ref::<crate::http::HttpError>() {
                    anyhow!(
                        "el addon respondió HTTP {} en manifest.json (¿token vencido?)",
                        he.status
                    )
                } else {
                    e
                }
            })?;
        if m.id.trim().is_empty() {
            return Err(anyhow!("manifest sin campo `id`"));
        }
        Ok(m)
    }

    pub fn search(&self, addon: &Addon, q: &str, net: NetworkKind) -> Result<SearchResponse> {
        let url = addon.url("search");
        let headers = addon.headers();
        let params = addon.settings_params(net);
        // caché corto: la misma búsqueda repetida (scroll/tecleo) no pega de nuevo
        let ttl = Duration::from_secs(30 * 60);
        crate::http::JsonRequest::new(&url)
            .query("q", q)
            .query_all(params)
            .headers(headers)
            .timeout(Duration::from_secs(20))
            .cache(&self.http.cache, ttl)
            .json::<SearchResponse>()
            .map(SearchResponse::normalized)
    }

    pub fn stream(&self, addon: &Addon, track_id: &str, net: NetworkKind) -> Result<StreamResponse> {
        let url = addon.url(&format!("stream/{}", pct(track_id)));
        let params = addon.settings_params(net);
        let headers = addon.headers();
        // El stream NO se cachea a disco por defecto: las URLs suelen expirar.
        let r = crate::http::JsonRequest::new(&url)
            .query_all(params)
            .headers(headers)
            .timeout(Duration::from_secs(25));
        Ok(r.json()?)
    }

    pub fn album(&self, addon: &Addon, id: &str, net: NetworkKind, ttl: Duration) -> Result<Album> {
        let url = addon.url(&format!("album/{}", pct(id)));
        Ok(crate::http::JsonRequest::new(&url)
            .query_all(addon.settings_params(net))
            .headers(addon.headers())
            .cache(&self.http.cache, ttl)
            .timeout(Duration::from_secs(20))
            .json::<Album>()?
            .normalized())
    }

    pub fn artist(&self, addon: &Addon, id: &str, net: NetworkKind, ttl: Duration) -> Result<Artist> {
        let url = addon.url(&format!("artist/{}", pct(id)));
        Ok(crate::http::JsonRequest::new(&url)
            .query_all(addon.settings_params(net))
            .headers(addon.headers())
            .cache(&self.http.cache, ttl)
            .timeout(Duration::from_secs(20))
            .json::<Artist>()?
            .normalized())
    }

    pub fn playlist(
        &self,
        addon: &Addon,
        id: &str,
        net: NetworkKind,
        ttl: Duration,
    ) -> Result<Playlist> {
        let url = addon.url(&format!("playlist/{}", pct(id)));
        Ok(crate::http::JsonRequest::new(&url)
            .query_all(addon.settings_params(net))
            .headers(addon.headers())
            .cache(&self.http.cache, ttl)
            .timeout(Duration::from_secs(25))
            .json::<Playlist>()?
            .normalized())
    }

    pub fn catalog(
        &self,
        addon: &Addon,
        catalog_id: &str,
        skip: u32,
        net: NetworkKind,
        ttl: Duration,
    ) -> Result<CatalogResponse> {
        let url = addon.url(&format!("catalog/{}", pct(catalog_id)));
        Ok(crate::http::JsonRequest::new(&url)
            .query("skip", skip.to_string())
            .query_all(addon.settings_params(net))
            .headers(addon.headers())
            .cache(&self.http.cache, ttl)
            .timeout(Duration::from_secs(20))
            .json::<CatalogResponse>()?
            .normalized())
    }

    /// Devuelve el JSON **crudo** de cualquier endpoint del addon.
    /// Es lo que usa el modo diagnóstico: muestra exactamente qué mandó el servidor.
    pub fn fetch_raw(
        &self,
        addon: &Addon,
        path: &str,
        query: &[(&str, String)],
        net: NetworkKind,
    ) -> Result<String> {
        let url = addon.url(path);
        let mut r = crate::http::JsonRequest::new(&url)
            .query_all(addon.settings_params(net))
            .headers(addon.headers())
            .timeout(Duration::from_secs(25));
        for (k, v) in query {
            r = r.query(*k, v.clone());
        }
        let bytes = r.bytes()?;
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(v) => Ok(serde_json::to_string_pretty(&v).unwrap_or_default()),
            Err(_) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        }
    }

    /// `GET /resolve-isrc?isrc=` -> id del track en este addon (o None).
    pub fn resolve_isrc(&self, addon: &Addon, isrc: &str, net: NetworkKind) -> Result<Option<String>> {
        let url = addon.url("resolve-isrc");
        let r: ResolveIsrcResponse = crate::http::JsonRequest::new(&url)
            .query("isrc", isrc)
            .query_all(addon.settings_params(net))
            .headers(addon.headers())
            .timeout(Duration::from_secs(15))
            .json()?;
        Ok(r.id().map(|s| s.to_string()))
    }

    /// `GET /resolve?isrc=&title=&artist=&durationMs=` -> item exacto (o None).
    pub fn resolve(
        &self,
        addon: &Addon,
        identity: &TrackIdentity,
        net: NetworkKind,
    ) -> Result<Option<CatalogItem>> {
        let url = addon.url("resolve");
        let mut r = crate::http::JsonRequest::new(&url)
            .query("title", &identity.title)
            .query("artist", &identity.artist);
        if let Some(i) = &identity.isrc {
            r = r.query("isrc", i);
        }
        if let Some(d) = identity.duration {
            r = r.query("durationMs", (d * 1000.0).round() as u64);
        }
        let res: ResolveResponse = r
            .query_all(addon.settings_params(net))
            .headers(addon.headers())
            .timeout(Duration::from_secs(15))
            .json()?;
        Ok(res.item)
    }

    /// Verifica que una URL de stream sigue viva.
    pub fn probe_stream(url: &str, headers: &[(String, String)]) -> Result<(u16, Option<u64>)> {
        let (status, len, _ct) = crate::http::probe(url, headers)?;
        if !(200..300).contains(&status) {
            return Err(anyhow!("la URL de audio respondió HTTP {status}"));
        }
        Ok((status, len))
    }
}

/// Identidad de una grabación, independiente del addon (para `/resolve`).
#[derive(Debug, Clone, Default)]
pub struct TrackIdentity {
    pub title: String,
    pub artist: String,
    pub isrc: Option<String>,
    pub duration: Option<f64>,
}

impl TrackIdentity {
    pub fn from_track(addon_id: &str, t: &Track) -> Self {
        let _ = addon_id;
        Self {
            title: t.title.clone(),
            artist: t.artist.clone(),
            isrc: t.isrc.clone(),
            duration: t.duration_secs(),
        }
    }
}

/// Percent-encode de un id para meterlo en un path.
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Utilidad compartida: filtra contenido explícito si el usuario lo pidió.
pub fn apply_explicit_filter(res: &mut SearchResponse, hide: bool) {
    if !hide {
        return;
    }
    res.tracks.retain(|t| t.explicit != Some(true));
    res.albums.retain(|a| a.explicit != Some(true));
    res.playlists.retain(|_| true);
}

/// Cache de manifests en memoria (por base url) para no re-parsear.
#[derive(Default)]
pub struct ManifestCache {
    map: RwLock<BTreeMap<String, (Manifest, std::time::Instant)>>,
}

impl std::fmt::Debug for ManifestCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManifestCache").finish()
    }
}

impl ManifestCache {
    pub fn get(&self, base: &str) -> Option<Manifest> {
        rg(&self.map).get(base).map(|(x, _)| x.clone())
    }
    pub fn put(&self, base: &str, m: Manifest) {
        wg(&self.map).insert(base.to_string(), (m, std::time::Instant::now()));
    }
    pub fn forget(&self, base: &str) {
        wg(&self.map).remove(base);
    }
    pub fn forget_all(&self) {
        wg(&self.map).clear();
    }
}

