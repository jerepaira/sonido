//! Enriquecimiento de portadas por ISRC.
//!
//! Muchos addons de música devuelven los metadatos pero **no** la portada
//! (o devuelven una chica). Eclipse resuelve esto consultando el catálogo de
//! Apple Music con MusicKit; acá hacemos lo equivalente con la API pública de
//! búsqueda de iTunes, que no necesita clave:
//!
//!     GET https://itunes.apple.com/search?term={isrc}&entity=song&limit=1
//!
//! y de ahí `artworkUrl100` → `artworkUrl600` (misma imagen, más resolución).
//!
//! Se usa **solo** como relleno: si el addon trae portada, gana el addon.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;

use crate::http::{DiskCache, JsonRequest};

const ITUNES: &str = "https://itunes.apple.com/search";
const TTL: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Debug, Deserialize)]
struct ItunesResponse {
    #[serde(default)]
    result_count: u32,
    #[serde(default)]
    results: Vec<ItunesSong>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // campos que vienen en la respuesta y no usamos todavía
struct ItunesSong {
    #[serde(default)]
    track_id: Option<u64>,
    #[serde(default)]
    track_name: Option<String>,
    #[serde(default)]
    artist_name: Option<String>,
    #[serde(default)]
    isrc: Option<String>,
    #[serde(default)]
    artwork_url100: Option<String>,
    #[serde(default)]
    artwork_url600: Option<String>,
    #[serde(default)]
    collection_name: Option<String>,
}

impl ItunesSong {
    /// Sube la resolución de la portada (100x100 -> 600x600).
    fn artwork(&self) -> Option<String> {
        if let Some(a) = &self.artwork_url600 {
            return Some(a.clone());
        }
        let a = self.artwork_url100.as_deref()?;
        Some(upscale_itunes_url(a))
    }
}

/// `.../100x100bb.jpg` -> `.../600x600bb.jpg`
pub fn upscale_itunes_url(url: &str) -> String {
    let re_sizes = ["100x100", "170x170", "200x200", "300x300", "60x60"];
    for s in re_sizes {
        if url.contains(s) {
            return url.replace(s, "600x600");
        }
    }
    url.to_string()
}

/// Busca la portada de un ISRC. Devuelve `None` si no la encuentra.
pub fn artwork_for_isrc(cache: &DiskCache, isrc: &str) -> Option<String> {
    let key = format!("art|isrc|{}", isrc.to_ascii_uppercase());
    if let Some((bytes, _)) = cache.get(&key, TTL) {
        let s = String::from_utf8_lossy(&bytes).to_string();
        if s.is_empty() {
            return None;
        }
        return Some(s);
    }

    let url = JsonRequest::new(ITUNES)
        .query("term", isrc)
        .query("entity", "song")
        .query("limit", "3")
        .timeout(Duration::from_secs(8))
        .bytes();

    let result = match url {
        Ok(bytes) => match serde_json::from_slice::<ItunesResponse>(&bytes) {
            Ok(r) => pick_best(&r, isrc),
            Err(e) => {
                tracing::debug!("itunes: JSON inválido para {isrc}: {e}");
                None
            }
        },
        Err(e) => {
            tracing::debug!("itunes: fallo para {isrc}: {e}");
            None
        }
    };

    cache.put(&key, result.as_deref().unwrap_or("").as_bytes());
    result
}

fn pick_best(r: &ItunesResponse, isrc: &str) -> Option<String> {
    if r.result_count == 0 || r.results.is_empty() {
        return None;
    }
    let want = isrc.to_ascii_uppercase();
    // 1) coincidencia exacta de ISRC
    if let Some(s) = r
        .results
        .iter()
        .find(|s| s.isrc.as_deref().map(|i| i.to_ascii_uppercase()) == Some(want.clone()))
    {
        return s.artwork();
    }
    // 2) el primero (iTunes ya rankea por relevancia del término = ISRC)
    r.results.first().and_then(|s| s.artwork())
}

/// Portada por nombre de álbum + artista (para álbumes sin ISRC).
pub fn artwork_for_album(cache: &DiskCache, album: &str, artist: &str) -> Option<String> {
    if album.trim().is_empty() {
        return None;
    }
    let key = format!(
        "art|album|{}|{}",
        album.to_ascii_lowercase(),
        artist.to_ascii_lowercase()
    );
    if let Some((bytes, _)) = cache.get(&key, TTL) {
        let s = String::from_utf8_lossy(&bytes).to_string();
        if s.is_empty() {
            return None;
        }
        return Some(s);
    }

    #[derive(Deserialize)]
    struct R {
        #[serde(default)]
        results: Vec<ItunesSong>,
    }

    let res: Result<Vec<u8>> = JsonRequest::new(ITUNES)
        .query("term", format!("{artist} {album}"))
        .query("entity", "album")
        .query("limit", "3")
        .timeout(Duration::from_secs(8))
        .bytes();

    let out = match res {
        Ok(bytes) => serde_json::from_slice::<R>(&bytes)
            .ok()
            .and_then(|r| r.results.first().and_then(|s| s.artwork())),
        Err(_) => None,
    };
    cache.put(&key, out.as_deref().unwrap_or("").as_bytes());
    out
}

/// Rellena portadas faltantes de una tanda de temas.
/// Devuelve `track_id -> url` solo para los que consiguieron algo.
pub fn enrich_tracks(
    cache: &DiskCache,
    addon_id: &str,
    tracks: &[(String, Option<String>)],
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (tid, isrc) in tracks {
        let Some(isrc) = isrc.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        if let Some(url) = artwork_for_isrc(cache, isrc) {
            out.insert(format!("{addon_id}::{tid}"), url);
        }
    }
    out
}
