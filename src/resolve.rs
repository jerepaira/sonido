//! Resolución de streams: cacheo, elección del mejor candidato y cadena de
//! fallback entre addons (ISRC -> `/resolve` -> búsqueda puntuada).

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::addon::{Addon, AddonApi, TrackIdentity};
use crate::models::*;
use crate::net::NetworkKind;

/// Umbral de confianza para aceptar un resultado de búsqueda como el tema pedido.
pub const MATCH_THRESHOLD: f64 = 0.62;

/// Tipo de manifiesto de streaming adaptativo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adaptive {
    None,
    Dash,
    Hls,
}

impl Adaptive {
    pub fn label(self) -> &'static str {
        match self {
            Adaptive::None => "archivo directo",
            Adaptive::Dash => "MPEG-DASH (.mpd)",
            Adaptive::Hls => "HLS (.m3u8)",
        }
    }
}

/// Detecta si la URL es un manifiesto DASH/HLS en vez de un archivo de audio.
///
/// Es exactamente lo que devuelven los addons de Tidal: una URL firmada a un
/// `.mpd` en `im-cf.manifest.tidal.com`. El audio real son segmentos que el
/// reproductor va bajando (y descifrando si hace falta) mientras avanza el tema.
pub fn adaptive_kind(r: &StreamResponse, url: &str) -> Adaptive {
    let m = r
        .manifest
        .as_deref()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    if m.contains("dash") {
        return Adaptive::Dash;
    }
    if m.contains("hls") || m.contains("m3u8") {
        return Adaptive::Hls;
    }
    let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    if path.ends_with(".mpd") {
        return Adaptive::Dash;
    }
    if path.ends_with(".m3u8") || path.ends_with(".m3u") {
        return Adaptive::Hls;
    }
    let ct = r
        .container
        .as_deref()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    if ct.contains("dash") || ct == "mpd" {
        return Adaptive::Dash;
    }
    if ct.contains("hls") {
        return Adaptive::Hls;
    }
    Adaptive::None
}

/// Qué pasó cuando probamos la URL de audio antes de mandarla al motor.
#[derive(Debug, Clone, PartialEq)]
pub enum Probe {
    /// No se probó (opción desactivada).
    Skipped,
    Ok {
        status: u16,
        bytes: Option<u64>,
        content_type: Option<String>,
        range_ok: bool,
        elapsed: Duration,
    },
    /// Aviso: puede que igual suene (algunos CDN rechazan HEAD/Range).
    Warn(String),
    /// Definitivamente no va a sonar.
    Fatal(String),
}

impl Probe {
    pub fn ok(&self) -> bool {
        !matches!(self, Probe::Fatal(_))
    }
    pub fn label(&self) -> String {
        match self {
            Probe::Skipped => "sin verificar".into(),
            Probe::Ok {
                status,
                bytes,
                content_type,
                range_ok,
                elapsed,
            } => format!(
                "HTTP {} · {} · {} · seek {} · {}",
                status,
                bytes
                    .map(crate::util::fmt_bytes)
                    .unwrap_or_else(|| "?".into()),
                content_type.clone().unwrap_or_else(|| "?".into()),
                crate::util::fmt_latency(*elapsed),
                if *range_ok {
                    "ok"
                } else {
                    "sin Range"
                }
            ),
            Probe::Warn(w) => format!("aviso: {w}"),
            Probe::Fatal(f) => format!("no reproducible: {f}"),
        }
    }
}

/// Prueba la URL de audio con un pedido chico y devuelve un diagnóstico preciso.
///
/// Es la diferencia entre "no suena" y "el CDN devolvió 403 porque el token
/// del addon expiró" — que es exactamente lo que pasa con streams descifrados
/// en vuelo cuya URL vence a los pocos minutos.
pub fn verify_stream(url: &str, headers: &[(String, String)]) -> Probe {
    verify_stream_adaptive(url, headers, Adaptive::None)
}

/// Como [`verify_stream`], pero sabiendo si es un manifiesto adaptativo: ahí no
/// se exige soporte de `Range` (el manifiesto es un XML chico) y el
/// content-type esperado es texto, no audio.
pub fn verify_stream_adaptive(
    url: &str,
    headers: &[(String, String)],
    adaptive: Adaptive,
) -> Probe {
    let t = Instant::now();
    match crate::http::probe_verbose(url, headers) {
        Ok(pr) => {
            let elapsed = t.elapsed();
            match pr.status {
                200 | 206 => Probe::Ok {
                    status: pr.status,
                    bytes: pr.content_length.or(pr.range_total),
                    content_type: pr.content_type,
                    range_ok: pr.range_ok || adaptive != Adaptive::None,
                    elapsed,
                },
                401 | 403 => Probe::Fatal(format!(
                    "HTTP {} — la URL necesita autorización o el token expiró (muy común con                      streams que se descifran en vuelo: pedí el tema de nuevo)",
                    pr.status
                )),
                404 => Probe::Fatal("HTTP 404 — la URL de audio no existe".into()),
                405 | 501 => Probe::Warn(format!(
                    "HTTP {} — el servidor no acepta HEAD/Range; puede que igual reproduzca",
                    pr.status
                )),
                s if (500..600).contains(&s) => {
                    Probe::Fatal(format!("HTTP {s} — error del servidor de audio"))
                }
                s => Probe::Warn(format!("HTTP {s} — estado inesperado")),
            }
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("timeout") {
                Probe::Warn(format!("timeout al verificar ({msg})"))
            } else {
                Probe::Fatal(msg)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedStream {
    pub addon_id: String,
    pub addon_name: String,
    pub track_id: String,
    pub url: String,
    pub stream: StreamResponse,
    pub headers: Vec<(String, String)>,
    pub score: i64,
    pub resolved_in: Duration,
    /// Cómo se consiguió (para mostrar en la UI / diagnósticos).
    pub via: Via,
    pub expires_at: Option<u64>,
    /// Verificación previa de la URL (diagnóstico).
    pub probe: Probe,
    /// DASH / HLS / archivo directo.
    pub adaptive: Adaptive,
    /// ISRC reportado por el stream (si el addon lo manda).
    pub isrc: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Direct,
    StreamUrl,
    Isrc,
    Resolve,
    SearchMatch,
    Cache,
}

impl Via {
    pub fn label(self) -> &'static str {
        match self {
            Via::Direct => "directo",
            Via::StreamUrl => "streamURL",
            Via::Isrc => "ISRC",
            Via::Resolve => "resolve",
            Via::SearchMatch => "búsqueda",
            Via::Cache => "caché",
        }
    }
}

// ──────────────────────────────── caché en memoria ────────────────────────────────

#[derive(Default)]
pub struct StreamCache {
    map: RwLock<BTreeMap<String, (ResolvedStream, Instant)>>,
}

impl StreamCache {
    pub fn get(&self, key: &str) -> Option<ResolvedStream> {
        let m = self.map.read().ok()?;
        let (s, at) = m.get(key)?;
        // 4 minutos de validez, o hasta el 80% del expiresAt del addon
        let age = at.elapsed();
        if age > Duration::from_secs(240) {
            return None;
        }
        if let Some(exp) = s.expires_at {
            let now = crate::config::now_secs();
            if exp.saturating_sub(now) < 20 {
                return None;
            }
        }
        Some(s.clone())
    }

    pub fn put(&self, key: &str, s: ResolvedStream) {
        if let Ok(mut m) = self.map.write() {
            m.insert(key.to_string(), (s, Instant::now()));
        }
    }

    pub fn clear(&self) {
        if let Ok(mut m) = self.map.write() {
            m.clear();
        }
    }

    pub fn len(&self) -> usize {
        self.map.read().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub fn cache_key(addon_id: &str, track_id: &str) -> String {
    format!("{addon_id}::{track_id}")
}

// ──────────────────────────────── resolución ────────────────────────────────

pub struct Resolver {
    pub cache: Arc<StreamCache>,
}

pub struct ResolvePrefs {
    pub prefer_hires: bool,
    pub prefer_atmos: bool,
    pub net: NetworkKind,
    /// Verificar la URL (HEAD/Range) antes de mandarla al motor.
    pub probe_stream: bool,
}

impl Resolver {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(StreamCache::default()),
        }
    }

    /// Resuelve el stream de un tema en un addon concreto.
    /// Si el track ya trae `streamURL`, se usa directo (cero round-trips).
    pub fn resolve_direct(
        &self,
        api: &AddonApi,
        addon: &Addon,
        track: &Track,
        prefs: &ResolvePrefs,
    ) -> Result<ResolvedStream> {
        let aid = addon.id();
        let key = cache_key(&aid, &track.id);
        if let Some(mut hit) = self.cache.get(&key) {
            hit.via = Via::Cache;
            return Ok(hit);
        }

        let t = Instant::now();

        if let Some(url) = track.stream_url.as_deref().filter(|s| !s.is_empty()) {
            let res = ResolvedStream {
                addon_id: aid.clone(),
                addon_name: addon.name(),
                track_id: track.id.clone(),
                url: url.to_string(),
                stream: StreamResponse {
                    url: Some(url.to_string()),
                    format: track.format.clone(),
                    ..Default::default()
                },
                headers: Vec::new(),
                score: 0,
                resolved_in: t.elapsed(),
                via: Via::StreamUrl,
                expires_at: None,
                probe: Probe::Skipped,
                adaptive: Adaptive::None,
                isrc: track.isrc.clone(),
            };
            self.cache.put(&key, res.clone());
            return Ok(res);
        }

        let s = api.stream(addon, &track.id, prefs.net)?;
        let url = s
            .url
            .clone()
            .ok_or_else(|| anyhow::anyhow!("el addon no devolvió URL de audio"))?;
        if url.trim().is_empty() {
            anyhow::bail!("el addon devolvió una URL vacía");
        }
        let headers = s.headers();
        let adaptive = adaptive_kind(&s, &url);
        let isrc = s.isrc.clone().or_else(|| track.isrc.clone());
        let score = s.score(prefs.prefer_atmos, prefs.prefer_hires);
        let res = ResolvedStream {
            addon_id: aid,
            addon_name: addon.name(),
            track_id: track.id.clone(),
            url,
            expires_at: s.expires_at,
            stream: s,
            headers,
            score,
            resolved_in: t.elapsed(),
            via: Via::Direct,
            probe: Probe::Skipped,
            adaptive,
            isrc,
        };
        self.cache.put(&key, res.clone());
        Ok(res)
    }

    /// Cadena de fallback: intenta que OTRO addon consiga la misma grabación.
    ///
    /// Orden por addon: `resolve-isrc` (si hay ISRC) -> `resolve` -> búsqueda puntuada.
    pub fn resolve_identity(
        &self,
        api: &AddonApi,
        addons: &[Addon],
        identity: &TrackIdentity,
        prefs: &ResolvePrefs,
    ) -> Result<ResolvedStream> {
        let mut last_err = anyhow::anyhow!("ningún addon pudo resolver el tema");
        for addon in addons {
            if !crate::addon::rg(&addon.manifest).can_stream() {
                continue;
            }
            let m = crate::addon::rg(&addon.manifest).clone();

            // 1) ISRC exacto
            if let Some(isrc) = identity.isrc.as_deref().filter(|_| m.has("isrc")) {
                if let Ok(Some(tid)) = api.resolve_isrc(addon, isrc, prefs.net) {
                    let track = Track {
                        id: tid,
                        title: identity.title.clone(),
                        artist: identity.artist.clone(),
                        ..Default::default()
                    };
                    match self.resolve_direct(api, addon, &track, prefs) {
                        Ok(mut r) => {
                            r.via = Via::Isrc;
                            return Ok(r);
                        }
                        Err(e) => last_err = e,
                    }
                }
            }

            // 2) /resolve
            if m.has("resolve") {
                if let Ok(Some(item)) = api.resolve(addon, identity, prefs.net) {
                    let track = item.to_track();
                    match self.resolve_direct(api, addon, &track, prefs) {
                        Ok(mut r) => {
                            r.via = Via::Resolve;
                            return Ok(r);
                        }
                        Err(e) => last_err = e,
                    }
                }
            }

            // 3) búsqueda + scoring
            if m.has("search") {
                let q = format!("{} {}", identity.artist, identity.title);
                if let Ok(res) = api.search(addon, &q, prefs.net) {
                    let mut best: Option<(f64, &Track)> = None;
                    for t in &res.tracks {
                        let sc = crate::util::match_score(
                            &identity.title,
                            &identity.artist,
                            identity.isrc.as_deref(),
                            identity.duration,
                            &t.title,
                            &t.artist,
                            t.isrc.as_deref(),
                            t.duration_secs(),
                        );
                        if best.map(|(b, _)| sc > b).unwrap_or(true) {
                            best = Some((sc, t));
                        }
                    }
                    if let Some((sc, t)) = best {
                        if sc >= MATCH_THRESHOLD {
                            match self.resolve_direct(api, addon, t, prefs) {
                                Ok(mut r) => {
                                    r.via = Via::SearchMatch;
                                    return Ok(r);
                                }
                                Err(e) => last_err = e,
                            }
                        } else {
                            last_err = anyhow::anyhow!(
                                "{}: mejor candidato con score {sc:.2} (< {MATCH_THRESHOLD})",
                                addon.name()
                            );
                        }
                    }
                }
            }
        }
        Err(last_err)
    }

    /// Resuelve una pista de la biblioteca (favoritos/playlists/recientes).
    pub fn resolve_library(
        &self,
        api: &AddonApi,
        addons: &[Addon],
        t: &LibraryTrack,
        prefs: &ResolvePrefs,
    ) -> Result<ResolvedStream> {
        // 1) el addon original
        if let Some(a) = addons.iter().find(|a| a.id() == t.addon_id) {
            let track = t.to_track();
            if let Ok(r) = self.resolve_direct(api, a, &track, prefs) {
                return Ok(r);
            }
        }
        // 2) URL directa guardada
        if let Some(url) = t.source_url.as_deref().filter(|s| !s.is_empty()) {
            return Ok(ResolvedStream {
                addon_id: t.addon_id.clone(),
                addon_name: t.addon_id.clone(),
                track_id: t.track_id.clone(),
                url: url.to_string(),
                stream: StreamResponse {
                    url: Some(url.to_string()),
                    ..Default::default()
                },
                headers: Vec::new(),
                score: 0,
                resolved_in: Duration::ZERO,
                via: Via::StreamUrl,
                expires_at: None,
                probe: Probe::Skipped,
                adaptive: Adaptive::None,
                isrc: t.isrc.clone(),
            });
        }
        // 3) cadena de fallback por identidad
        let id = TrackIdentity {
            title: t.title.clone(),
            artist: t.artist.clone(),
            isrc: t.isrc.clone(),
            duration: t.duration,
        };
        self.resolve_identity(api, addons, &id, prefs)
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}
