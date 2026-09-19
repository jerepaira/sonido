//! Modelo de datos del protocolo de addons de Eclipse Music.
//!
//! Spec: <https://eclipsemusic.app/docs>
//!
//! Endpoints que consume la app:
//!   GET /manifest.json
//!   GET /search?q={query}
//!   GET /stream/{trackId}
//!   GET /album/{id} | /artist/{id} | /playlist/{id}
//!   GET /resolve-isrc?isrc={isrc}
//!   GET /resolve?isrc=&title=&artist=&durationMs=
//!   GET /catalog/{id}?skip={n}
//!
//! Además, los `settings` declarados en el manifest se envían como query params
//! en **todas** las peticiones.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

// ─────────────────────────── helpers de deserialización ───────────────────────
// Los addons reales son generosos con los tipos: duraciones en segundos o en
// milisegundos, números como string, campos ausentes... Acá se normaliza todo.

pub fn opt_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Null => None,
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_owned())
            }
        }
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        other => Some(other.to_string()),
    })
}

pub fn empty_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(opt_string(d)?.unwrap_or_default())
}

pub fn opt_bool<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Null => None,
        Value::Bool(b) => Some(b),
        Value::Number(n) => n.as_i64().map(|v| v != 0),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "y" => Some(true),
            "false" | "0" | "no" | "n" => Some(false),
            _ => None,
        },
        _ => None,
    })
}

pub fn opt_f64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Null => None,
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Bool(b) => Some(if b { 1.0 } else { 0.0 }),
        _ => None,
    })
}

pub fn opt_u32<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    Ok(opt_f64(d)?.and_then(|v| {
        if v.is_finite() && v >= 0.0 {
            Some(v as u32)
        } else {
            None
        }
    }))
}

pub fn opt_u64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Ok(opt_f64(d)?.and_then(|v| {
        if v.is_finite() && v >= 0.0 {
            Some(v as u64)
        } else {
            None
        }
    }))
}

pub fn string_vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Null => Vec::new(),
        Value::Array(a) => a
            .into_iter()
            .filter_map(|v| match v {
                Value::String(s) if !s.trim().is_empty() => Some(s),
                Value::String(_) => None,
                other if !other.is_null() => Some(other.to_string()),
                _ => None,
            })
            .collect(),
        Value::String(s) => s
            .split(',')
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .collect(),
        _ => Vec::new(),
    })
}

pub fn value_map<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, Value>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Object(m) => m.into_iter().collect(),
        _ => BTreeMap::new(),
    })
}

/// Duración -> segundos. Acepta segundos o milisegundos indistintamente.
/// (`durationMs` entra por `alias` en el mismo campo, así que se normaliza igual.)
pub fn opt_duration_secs<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(opt_f64(d)?.map(seconds_from).filter(|v| *v > 0.0))
}

/// Segundos, tolerante a milisegundos.
/// Heurística: cualquier valor >= 10_000 se interpreta como ms.
pub fn seconds_from(v: f64) -> f64 {
    if v >= 10_000.0 {
        v / 1000.0
    } else {
        v
    }
}

pub fn opt_seconds<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(opt_f64(d)?.map(seconds_from).filter(|v| *v > 0.0))
}

/// Los addons no se ponen de acuerdo con los nombres de campo. Esta función
/// busca el primero presente entre varios alias (artworkURL, image, cover, …).
pub fn pick_alias(
    extra: &std::collections::BTreeMap<String, Value>,
    names: &[&str],
) -> Option<String> {
    for n in names {
        if let Some(v) = extra.get(*n) {
            let s: Option<String> = match v {
                Value::String(x) => Some(x.trim().to_string()),
                Value::Object(m) => m
                    .get("url")
                    .or_else(|| m.get("URL"))
                    .and_then(|x| x.as_str())
                    .map(|x| x.trim().to_string()),
                Value::Array(a) => a.iter().find_map(|x| match x {
                    Value::String(y) => Some(y.trim().to_string()),
                    Value::Object(m) => m
                        .get("url")
                        .and_then(|u| u.as_str())
                        .map(|y| y.trim().to_string()),
                    _ => None,
                }),
                _ => None,
            };
            if let Some(s) = s.filter(|s| !s.is_empty() && s != "null") {
                return Some(s);
            }
        }
    }
    None
}

/// Nombres de campo habituales para la portada.
pub const ARTWORK_ALIASES: &[&str] = &[
    "artworkUrl",
    "artwork",
    "artworkURL",
    "imageUrl",
    "imageURL",
    "image",
    "cover",
    "coverUrl",
    "coverURL",
    "coverArt",
    "thumbnail",
    "thumb",
    "picture",
    "poster",
    "albumArt",
    "albumArtwork",
];

/// Rellena `artwork_url` desde cualquier alias conocido.
pub fn fill_artwork(
    slot: &mut Option<String>,
    extra: &std::collections::BTreeMap<String, Value>,
) {
    if slot.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
        return;
    }
    *slot = pick_alias(extra, ARTWORK_ALIASES);
}

/// ISRC: un string vacío NO es un ISRC (lo dice la spec).
pub fn opt_isrc<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(opt_string(d)?.and_then(|s| {
        let t = s.trim().to_ascii_uppercase();
        if t.len() >= 8 {
            Some(t)
        } else {
            None
        }
    }))
}

// ──────────────────────────────── manifest ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub name: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub version: String,
    #[serde(default, deserialize_with = "opt_string")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub icon: Option<String>,
    #[serde(default, deserialize_with = "string_vec")]
    pub types: Vec<String>,
    #[serde(default, deserialize_with = "string_vec")]
    pub resources: Vec<String>,
    /// "music" | "audiobook" | "podcast"
    #[serde(default, deserialize_with = "opt_string")]
    pub content_type: Option<String>,
    #[serde(default)]
    pub settings: Vec<SettingDef>,
    #[serde(default)]
    pub catalogs: Vec<CatalogDef>,
}

impl Manifest {
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            self.id.clone()
        } else {
            self.name.clone()
        }
    }

    pub fn has(&self, resource: &str) -> bool {
        self.resources.iter().any(|r| r.eq_ignore_ascii_case(resource))
    }

    pub fn has_type(&self, t: &str) -> bool {
        self.types.iter().any(|x| x.eq_ignore_ascii_case(t))
    }

    pub fn can_search(&self) -> bool {
        self.has("search") || self.has("catalog")
    }

    pub fn can_stream(&self) -> bool {
        self.has("stream") || self.has("search")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SettingType {
    Select,
    Toggle,
    Text,
    Number,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingOption {
    #[serde(default, deserialize_with = "empty_string")]
    pub value: String,
    #[serde(default, deserialize_with = "opt_string")]
    pub label: Option<String>,
}

impl SettingOption {
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingDef {
    pub key: String,
    #[serde(rename = "type", default = "default_setting_type")]
    pub kind: SettingType,
    #[serde(default, deserialize_with = "empty_string")]
    pub label: String,
    #[serde(default, deserialize_with = "opt_string")]
    pub help: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub options: Vec<SettingOption>,
    #[serde(default = "default_true")]
    pub per_network: bool,
    /// El default puede venir como bool, número o string.
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default, deserialize_with = "opt_f64")]
    pub min: Option<f64>,
    #[serde(default, deserialize_with = "opt_f64")]
    pub max: Option<f64>,
    #[serde(default, deserialize_with = "opt_f64")]
    pub step: Option<f64>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub max_length: Option<u32>,
}

fn default_setting_type() -> SettingType {
    SettingType::Text
}

fn default_true() -> bool {
    true
}

impl SettingDef {
    pub fn label(&self) -> &str {
        if self.label.trim().is_empty() {
            &self.key
        } else {
            &self.label
        }
    }

    /// Valor por defecto serializado como se lo manda por query param.
    pub fn default_str(&self) -> Option<String> {
        match &self.default {
            None => None,
            Some(Value::Null) => None,
            Some(Value::Bool(b)) => Some(if *b { "true".into() } else { "false".into() }),
            Some(Value::String(s)) if s.is_empty() => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            Some(v) => Some(v.to_string()),
        }
    }

    pub fn default_bool(&self) -> bool {
        matches!(&self.default, Some(Value::Bool(true)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogDef {
    #[serde(default, deserialize_with = "empty_string")]
    pub id: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub r#type: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub name: String,
}

// ──────────────────────────────── items ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Track,
    Album,
    Artist,
    Playlist,
    #[serde(other)]
    #[default]
    Unknown,
}

impl ItemKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemKind::Track => "track",
            ItemKind::Album => "album",
            ItemKind::Artist => "artist",
            ItemKind::Playlist => "playlist",
            ItemKind::Unknown => "unknown",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "track" | "song" | "tracks" => ItemKind::Track,
            "album" | "albums" => ItemKind::Album,
            "artist" | "artists" => ItemKind::Artist,
            "playlist" | "playlists" => ItemKind::Playlist,
            _ => ItemKind::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    #[serde(default, deserialize_with = "empty_string")]
    pub id: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub title: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub artist: String,
    #[serde(default, deserialize_with = "opt_string")]
    pub album: Option<String>,
    /// Segundos (normalizado). Acepta `duration` (s) y `durationMs` (ms).
    #[serde(
        default,
        alias = "durationMs",
        deserialize_with = "opt_duration_secs"
    )]
    pub duration: Option<f64>,
    #[serde(default, deserialize_with = "opt_string")]
    pub artwork_url: Option<String>,
    #[serde(default, deserialize_with = "opt_isrc")]
    pub isrc: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub format: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub stream_url: Option<String>,
    #[serde(default, deserialize_with = "opt_bool")]
    pub explicit: Option<bool>,
    #[serde(default, deserialize_with = "opt_bool")]
    pub hi_res: Option<bool>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub track_number: Option<u32>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub disc_number: Option<u32>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub year: Option<u32>,
    /// Cualquier campo extra que mande el addon (para debugging / futuro).
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Track {
    /// Los addons usan mil nombres para la portada; se normaliza al construir.
    pub fn normalized(mut self) -> Self {
        fill_artwork(&mut self.artwork_url, &self.extra);
        if self.isrc.is_none() {
            self.isrc = pick_alias(&self.extra, &["isrc", "ISRC", "isrcCode"]).and_then(|s| {
                let t = s.trim().to_ascii_uppercase();
                if t.len() >= 8 { Some(t) } else { None }
            });
        }
        self
    }

    pub fn duration_secs(&self) -> Option<f64> {
        self.duration
    }

    pub fn subtitle(&self) -> String {
        match &self.album {
            Some(a) if !a.is_empty() => format!("{} · {}", self.artist, a),
            _ => self.artist.clone(),
        }
    }

    pub fn artwork(&self) -> Option<&str> {
        self.artwork_url.as_deref().filter(|s| !s.is_empty())
    }

    /// Clave dedup: ISRC si hay, si no título+artista normalizado.
    pub fn dedup_key(&self) -> String {
        if let Some(i) = &self.isrc {
            return format!("isrc:{i}");
        }
        format!(
            "ta:{}:{}",
            crate::util::fold_ascii(&self.title),
            crate::util::fold_ascii(&self.artist)
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    #[serde(default, deserialize_with = "empty_string")]
    pub id: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub title: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub artist: String,
    #[serde(default, deserialize_with = "opt_string")]
    pub artwork_url: Option<String>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub track_count: Option<u32>,
    #[serde(default, deserialize_with = "opt_string")]
    pub year: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "opt_bool")]
    pub explicit: Option<bool>,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Album {
    pub fn normalized(mut self) -> Self {
        fill_artwork(&mut self.artwork_url, &self.extra);
        self.tracks = self.tracks.into_iter().map(Track::normalized).collect();
        self
    }

    pub fn artwork(&self) -> Option<&str> {
        self.artwork_url.as_deref().filter(|s| !s.is_empty())
    }
    pub fn year_str(&self) -> String {
        self.year.clone().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    #[serde(default, deserialize_with = "empty_string")]
    pub id: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub name: String,
    #[serde(default, deserialize_with = "opt_string")]
    pub artwork_url: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub bio: Option<String>,
    #[serde(default, deserialize_with = "string_vec")]
    pub genres: Vec<String>,
    #[serde(default)]
    pub top_tracks: Vec<Track>,
    #[serde(default)]
    pub albums: Vec<Album>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Artist {
    pub fn normalized(mut self) -> Self {
        fill_artwork(&mut self.artwork_url, &self.extra);
        self.top_tracks = self.top_tracks.into_iter().map(Track::normalized).collect();
        self.albums = self.albums.into_iter().map(Album::normalized).collect();
        self
    }

    pub fn artwork(&self) -> Option<&str> {
        self.artwork_url.as_deref().filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    #[serde(default, deserialize_with = "empty_string")]
    pub id: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub title: String,
    #[serde(default, deserialize_with = "opt_string")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub artwork_url: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub creator: Option<String>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub track_count: Option<u32>,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Playlist {
    pub fn normalized(mut self) -> Self {
        fill_artwork(&mut self.artwork_url, &self.extra);
        self.tracks = self.tracks.into_iter().map(Track::normalized).collect();
        self
    }

    pub fn artwork(&self) -> Option<&str> {
        self.artwork_url.as_deref().filter(|s| !s.is_empty())
    }
}

// ──────────────────────────────── search ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchResponse {
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub albums: Vec<Album>,
    #[serde(default)]
    pub artists: Vec<Artist>,
    #[serde(default)]
    pub playlists: Vec<Playlist>,
}

impl SearchResponse {
    /// Aplica la normalización de campos (portadas, ISRC) a todo el resultado.
    pub fn normalized(mut self) -> Self {
        self.tracks = self.tracks.into_iter().map(Track::normalized).collect();
        self.albums = self.albums.into_iter().map(Album::normalized).collect();
        self.artists = self.artists.into_iter().map(Artist::normalized).collect();
        self.playlists = self.playlists.into_iter().map(Playlist::normalized).collect();
        self
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
    }
    pub fn total(&self) -> usize {
        self.tracks.len() + self.albums.len() + self.artists.len() + self.playlists.len()
    }
}

// ──────────────────────────────── stream ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamVideo {
    #[serde(default, deserialize_with = "opt_string")]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub mime_type: Option<String>,
    #[serde(default, deserialize_with = "opt_bool")]
    pub muxed: Option<bool>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub width: Option<u32>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub height: Option<u32>,
    #[serde(default, deserialize_with = "opt_bool")]
    pub independent: Option<bool>,
    #[serde(default)]
    pub renditions: Vec<VideoRendition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VideoRendition {
    #[serde(default, deserialize_with = "empty_string")]
    pub url: String,
    #[serde(default, deserialize_with = "opt_u32")]
    pub height: Option<u32>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub width: Option<u32>,
    #[serde(default, deserialize_with = "opt_string")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Chapter {
    #[serde(default, deserialize_with = "empty_string")]
    pub title: String,
    /// Segundos (normalizado: la spec dice segundos).
    #[serde(default, deserialize_with = "opt_seconds")]
    pub start_time: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamResponse {
    #[serde(
        default,
        alias = "streamUrl",
        alias = "streamURL",
        alias = "audioUrl",
        alias = "audioURL",
        alias = "playUrl",
        alias = "playURL",
        alias = "file",
        alias = "src",
        deserialize_with = "opt_string"
    )]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub format: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub quality: Option<String>,
    #[serde(default, deserialize_with = "opt_u64")]
    pub expires_at: Option<u64>,
    #[serde(default, deserialize_with = "opt_string")]
    pub codec: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub container: Option<String>,
    /// "none" | "hls" | "dash"
    #[serde(default, deserialize_with = "opt_string")]
    pub manifest: Option<String>,
    #[serde(default, deserialize_with = "opt_bool")]
    pub encrypted: Option<bool>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub sample_rate: Option<u32>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub bit_depth: Option<u32>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub channels: Option<u32>,
    #[serde(default, deserialize_with = "opt_u32")]
    pub bitrate: Option<u32>,
    /// Algunos addons (los de Tidal, p.ej.) devuelven el ISRC junto al stream.
    #[serde(default, deserialize_with = "opt_isrc")]
    pub isrc: Option<String>,
    #[serde(default)]
    pub chapters: Vec<Chapter>,
    #[serde(default)]
    pub video: Option<StreamVideo>,
    /// Campos de routing extra (algunos addons mandan `headers`, `title`, etc.)
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Campos donde los addons suelen esconder los headers del stream.
pub const HEADER_KEYS: &[&str] = &[
    "headers",
    "streamHeaders",
    "audioHeaders",
    "requestHeaders",
    "httpHeaders",
    "responseHeaders",
];

fn headers_from_value(v: &Value, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                if let Some(s) = match val {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                } {
                    out.push((k.clone(), s));
                }
            }
        }
        Value::Array(a) => {
            // [{"name": "...", "value": "..."}]
            for it in a {
                if let Value::Object(m) = it {
                    let k = m
                        .get("name")
                        .or_else(|| m.get("key"))
                        .or_else(|| m.get("header"))
                        .and_then(|x| x.as_str());
                    let v = m
                        .get("value")
                        .or_else(|| m.get("val"))
                        .and_then(|x| x.as_str());
                    if let (Some(k), Some(v)) = (k, v) {
                        out.push((k.to_string(), v.to_string()));
                    }
                }
            }
        }
        Value::String(s) => {
            // "Key: Value" suelto
            if let Some((k, v)) = s.split_once(':') {
                out.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        _ => {}
    }
}

impl StreamResponse {
    /// Headers HTTP que el addon pide para la URL de audio. Se buscan en varios
    /// nombres posibles (`headers`, `streamHeaders`, `httpHeaders`, …), en
    /// formato mapa, lista de pares o string "K: V".
    pub fn headers(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for k in HEADER_KEYS {
            if let Some(v) = self.extra.get(*k) {
                headers_from_value(v, &mut out);
            }
        }
        if let Some(Value::String(ua)) = self.extra.get("userAgent") {
            out.push(("User-Agent".into(), ua.clone()));
        }
        if let Some(Value::String(r)) = self.extra.get("referer").or_else(|| self.extra.get("referrer")) {
            out.push(("Referer".into(), r.clone()));
        }
        if let Some(Value::String(o)) = self.extra.get("origin") {
            out.push(("Origin".into(), o.clone()));
        }
        // sin duplicados (gana el primero)
        let mut seen = std::collections::HashSet::new();
        out.retain(|(k, _)| seen.insert(k.to_ascii_lowercase()));
        out
    }

    /// Opciones extra para el motor (algunos addons las necesitan para streams
    /// descifrados en vuelo: demuxer, probesize, etc.)
    pub fn player_options(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for key in ["mpvOptions", "playerOptions", "ffmpegOptions", "demuxerOptions"] {
            if let Some(v) = self.extra.get(key) {
                match v {
                    Value::Object(m) => {
                        for (k, val) in m {
                            if let Value::String(s) = val {
                                out.push((k.clone(), s.clone()));
                            } else if let Some(s) = val.as_str() {
                                out.push((k.clone(), s.to_string()));
                            }
                        }
                    }
                    Value::Array(a) => {
                        for it in a {
                            if let Value::String(s) = it {
                                if let Some((k, v)) = s.split_once('=') {
                                    out.push((k.trim().to_string(), v.trim().to_string()));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// JSON crudo del stream, para el panel de diagnóstico.
    pub fn raw_json(&self) -> String {
        let mut m = serde_json::Map::new();
        if let Some(u) = &self.url {
            m.insert("url".into(), Value::String(u.clone()));
        }
        for (k, v) in &self.extra {
            m.insert(k.clone(), v.clone());
        }
        for (k, v) in [
            ("format", self.format.clone()),
            ("quality", self.quality.clone()),
            ("codec", self.codec.clone()),
            ("container", self.container.clone()),
            ("manifest", self.manifest.clone()),
        ] {
            if let Some(v) = v {
                m.insert(k.into(), Value::String(v));
            }
        }
        if let Some(b) = self.bit_depth {
            m.insert("bitDepth".into(), Value::from(b));
        }
        if let Some(sr) = self.sample_rate {
            m.insert("sampleRate".into(), Value::from(sr));
        }
        if let Some(e) = self.encrypted {
            m.insert("encrypted".into(), Value::from(e));
        }
        serde_json::to_string_pretty(&Value::Object(m)).unwrap_or_default()
    }

    /// Etiqueta corta para mostrar en la UI: "FLAC 24/96", "AAC 320k", ...
    pub fn badge(&self) -> String {
        let codec = self
            .codec
            .clone()
            .or_else(|| self.format.clone())
            .unwrap_or_default()
            .to_ascii_uppercase();
        let mut parts: Vec<String> = Vec::new();
        if !codec.is_empty() {
            parts.push(match codec.as_str() {
                "EAC3_JOC" | "EAC3" | "AC4" => "ATMOS".to_string(),
                other => other.to_string(),
            });
        }
        match (self.bit_depth, self.sample_rate) {
            (Some(b), Some(s)) if s > 0 => parts.push(format!("{b}/{s}kHz")),
            _ => {}
        }
        if parts.is_empty() {
            if let Some(q) = &self.quality {
                return q.clone();
            }
        }
        if let Some(q) = &self.quality {
            if !parts.iter().any(|p| p.eq_ignore_ascii_case(q)) {
                parts.push(q.clone());
            }
        }
        parts.join(" · ")
    }

    pub fn is_atmos(&self) -> bool {
        matches!(
            self.codec.as_deref().map(|c| c.to_ascii_lowercase()),
            Some(ref c) if c.contains("joc") || c.contains("atmos") || c == "eac3" || c == "ac4"
        )
    }

    /// Puntaje para ordenar candidatos (mayor = mejor).
    pub fn score(&self, prefer_atmos: bool, prefer_hires: bool) -> i64 {
        let mut s: i64 = 0;
        let codec = self.codec.clone().unwrap_or_default().to_ascii_lowercase();
        if codec.contains("flac") || codec.contains("alac") || codec.contains("pcm") {
            s += 400;
        } else if codec.contains("joc") || codec.contains("atmos") || codec == "eac3" {
            s += if prefer_atmos { 520 } else { 260 };
        } else if codec.contains("opus") {
            s += 220;
        } else if codec.contains("aac") || codec.contains("mp4") || codec.contains("m4a") {
            s += 160;
        } else if codec.contains("mp3") {
            s += 120;
        } else if codec.contains("ogg") || codec.contains("vorbis") {
            s += 140;
        }
        if let Some(b) = self.bit_depth {
            if b >= 24 {
                s += if prefer_hires { 220 } else { 60 };
            } else if b == 16 {
                s += 80;
            }
        }
        if let Some(sr) = self.sample_rate {
            s += match sr {
                0..=44_100 => 10,
                44_101..=48_000 => 20,
                48_001..=96_000 => 45,
                _ => 60,
            };
        }
        if let Some(br) = self.bitrate {
            s += (br.min(2000) / 40) as i64;
        }
        s
    }
}

// ──────────────────────────────── resolve ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResolveIsrcResponse {
    #[serde(default, deserialize_with = "opt_string")]
    pub track_id: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub id: Option<String>,
}

impl ResolveIsrcResponse {
    pub fn id(&self) -> Option<&str> {
        self.track_id
            .as_deref()
            .or(self.id.as_deref())
            .filter(|s| !s.trim().is_empty() && !s.eq_ignore_ascii_case("null"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogItem {
    #[serde(default, deserialize_with = "empty_string")]
    pub id: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub r#type: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub title: String,
    #[serde(default, deserialize_with = "empty_string")]
    pub artist: String,
    #[serde(default, deserialize_with = "opt_string")]
    pub album: Option<String>,
    #[serde(default, deserialize_with = "opt_u64")]
    pub duration_ms: Option<u64>,
    #[serde(default, deserialize_with = "opt_isrc")]
    pub isrc: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub artwork_url: Option<String>,
    #[serde(default, deserialize_with = "opt_bool")]
    pub explicit: Option<bool>,
    #[serde(default, deserialize_with = "opt_bool")]
    pub hi_res: Option<bool>,
    #[serde(default, deserialize_with = "opt_string")]
    pub year: Option<String>,
    #[serde(default, deserialize_with = "opt_string")]
    pub creator: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CatalogItem {
    pub fn normalized(mut self) -> Self {
        fill_artwork(&mut self.artwork_url, &self.extra);
        self
    }

    pub fn kind(&self) -> ItemKind {
        ItemKind::parse(&self.r#type)
    }
    pub fn duration_secs(&self) -> Option<f64> {
        self.duration_ms.map(|d| d as f64 / 1000.0)
    }
    pub fn artwork(&self) -> Option<&str> {
        self.artwork_url.as_deref().filter(|s| !s.is_empty())
    }
    pub fn to_track(&self) -> Track {
        Track {
            id: self.id.clone(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            duration: self.duration_secs(),
            artwork_url: self.artwork_url.clone(),
            isrc: self.isrc.clone(),
            explicit: self.explicit,
            hi_res: self.hi_res,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogResponse {
    #[serde(default)]
    pub items: Vec<CatalogItem>,
}

impl CatalogResponse {
    pub fn normalized(mut self) -> Self {
        self.items = self.items.into_iter().map(CatalogItem::normalized).collect();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResolveResponse {
    #[serde(default)]
    pub item: Option<CatalogItem>,
}

// ─────────────────────────── items de biblioteca local ───────────────────────────

/// Una pista tal como la guarda la biblioteca local (favoritos / playlists).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryTrack {
    pub addon_id: String,
    pub track_id: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub artwork_url: Option<String>,
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    /// URL directa si el addon la dio (permite reproducir sin el addon instalado).
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub added_at: u64,
}

impl LibraryTrack {
    pub fn from(addon_id: &str, t: &Track) -> Self {
        Self {
            addon_id: addon_id.to_string(),
            track_id: t.id.clone(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            artwork_url: t.artwork_url.clone(),
            isrc: t.isrc.clone(),
            duration: t.duration_secs(),
            source_url: t.stream_url.clone(),
            added_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }
    pub fn key(&self) -> String {
        format!("{}::{}", self.addon_id, self.track_id)
    }
    pub fn to_track(&self) -> Track {
        Track {
            id: self.track_id.clone(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            duration: self.duration,
            artwork_url: self.artwork_url.clone(),
            isrc: self.isrc.clone(),
            stream_url: self.source_url.clone(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalPlaylist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub tracks: Vec<LibraryTrack>,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}
