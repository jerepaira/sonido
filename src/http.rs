//! Capa HTTP: agente `ureq` con pool de conexiones, caché a disco y métricas.
//!
//! Todo es bloqueante a propósito: se ejecuta dentro de `tokio::task::spawn_blocking`
//! (o en el pool propio de la app), lo que evita arrastrar hyper/tower/h2 y deja
//! el binario chico y el arranque instantáneo.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

pub const USER_AGENT: &str = concat!("Sonido/", env!("CARGO_PKG_VERSION"), " (Linux; +native)");

/// Límite duro por respuesta de addon (los JSON nunca llegan a esto).
const MAX_BODY: u64 = 32 * 1024 * 1024;

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let cfg = ureq::Agent::config_builder()
            .http_status_as_error(false) // manejamos 404/403 a mano
            .max_redirects(8)
            .user_agent(USER_AGENT)
            .no_delay(true) // TCP_NODELAY: menos latencia en requests chicos
            .max_idle_connections(64)
            .max_idle_connections_per_host(8)
            .max_idle_age(Duration::from_secs(120))
            .input_buffer_size(64 * 1024)
            .output_buffer_size(64 * 1024)
            .max_response_header_size(256 * 1024)
            .timeout_connect(Some(Duration::from_secs(8)))
            .timeout_recv_response(Some(Duration::from_secs(15)))
            .timeout_global(Some(Duration::from_secs(35)))
            .build();
        cfg.new_agent()
    })
}

/// Permite calentar el pool de conexiones / TLS en el arranque.
pub fn warmup() {
    let _ = agent();
}

// ─────────────────────────────── métricas ───────────────────────────────

#[derive(Debug, Default, Clone)]
pub struct HostStats {
    pub requests: u64,
    pub errors: u64,
    pub bytes: u64,
    pub last_ms: u64,
    pub avg_ms: u64,
    pub total_ms: u64,
}

#[derive(Debug, Default)]
pub struct HttpStats {
    pub requests: AtomicU64,
    pub errors: AtomicU64,
    pub bytes: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cache_miss: AtomicU64,
    hosts: Mutex<HashMap<String, HostStats>>,
}

impl HttpStats {
    pub fn snapshot(&self) -> (Totals, Vec<(String, HostStats)>) {
        let mut hosts: Vec<_> = self
            .hosts
            .lock()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        hosts.sort_by(|a, b| b.1.requests.cmp(&a.1.requests));
        (
            Totals {
                requests: self.requests.load(Ordering::Relaxed),
                errors: self.errors.load(Ordering::Relaxed),
                bytes: self.bytes.load(Ordering::Relaxed),
                cache_hits: self.cache_hits.load(Ordering::Relaxed),
                cache_miss: self.cache_miss.load(Ordering::Relaxed),
            },
            hosts,
        )
    }

    fn record(&self, host: &str, ok: bool, bytes: u64, ms: u64) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        if !ok {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        if let Ok(mut m) = self.hosts.lock() {
            let e = m.entry(host.to_string()).or_default();
            e.requests += 1;
            if !ok {
                e.errors += 1;
            }
            e.bytes += bytes;
            e.last_ms = ms;
            e.total_ms += ms;
            e.avg_ms = e.total_ms / e.requests.max(1);
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Totals {
    pub requests: u64,
    pub errors: u64,
    pub bytes: u64,
    pub cache_hits: u64,
    pub cache_miss: u64,
}

pub fn stats() -> &'static HttpStats {
    static S: OnceLock<HttpStats> = OnceLock::new();
    S.get_or_init(HttpStats::default)
}

// ──────────────────────────────── caché a disco ────────────────────────────────

#[derive(Clone)]
pub struct DiskCache {
    root: PathBuf,
}

impl DiskCache {
    pub fn new(root: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&root);
        Self { root }
    }

    fn path_for(&self, key: &str) -> PathBuf {
        // sharding por los 2 primeros chars del hash para no tener 100k archivos en un dir
        let h = hash_key(key);
        self.root.join(&h[..2]).join(&h[2..4]).join(format!("{h}.bin"))
    }

    /// Devuelve (bytes, edad) si existe y no venció.
    pub fn get(&self, key: &str, max_age: Duration) -> Option<(Vec<u8>, Duration)> {
        let p = self.path_for(key);
        let data = std::fs::read(&p).ok()?;
        if data.len() < 17 {
            return None;
        }
        let secs = u64::from_be_bytes(data[0..8].try_into().ok()?);
        let len = u64::from_be_bytes(data[8..16].try_into().ok()?) as usize;
        let body = data.get(17..17 + len)?;
        let now = unix_now();
        if max_age != Duration::ZERO && secs + max_age.as_secs() < now {
            let _ = std::fs::remove_file(&p);
            return None;
        }
        let age = Duration::from_secs(now.saturating_sub(secs));
        stats().cache_hits.fetch_add(1, Ordering::Relaxed);
        Some((body.to_vec(), age))
    }

    pub fn put(&self, key: &str, body: &[u8]) {
        stats().cache_miss.fetch_add(1, Ordering::Relaxed);
        let p = self.path_for(key);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut buf = Vec::with_capacity(17 + body.len());
        buf.extend_from_slice(&unix_now().to_be_bytes());
        buf.extend_from_slice(&(body.len() as u64).to_be_bytes());
        buf.push(0);
        buf.extend_from_slice(body);
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, &buf).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }

    pub fn forget(&self, key: &str) {
        let _ = std::fs::remove_file(self.path_for(key));
    }

    /// Borra lo más viejo si el caché supera `max_bytes`.
    pub fn prune(&self, max_bytes: u64) {
        let mut files: Vec<(PathBuf, u64, u128)> = Vec::new();
        let mut total = 0u64;
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(md) = p.metadata() {
                    total += md.len();
                    files.push((
                        p,
                        md.len(),
                        md.modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_millis())
                            .unwrap_or(0),
                    ));
                }
            }
        }
        if total <= max_bytes {
            return;
        }
        files.sort_by_key(|f| f.2); // más viejos primero
        let mut freed = 0u64;
        let target = total - max_bytes;
        for (p, len, _) in files {
            if freed >= target {
                break;
            }
            if std::fs::remove_file(&p).is_ok() {
                freed += len;
            }
        }
        tracing::debug!("caché: liberados {}", crate::util::fmt_bytes(freed));
    }

    pub fn size_bytes(&self) -> u64 {
        let mut total = 0u64;
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if let Ok(md) = p.metadata() {
                        total += md.len();
                    }
                }
            }
        }
        total
    }

    pub fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::create_dir_all(&self.root);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// FNV-1a 64 + hex (barato, suficiente para nombres de archivo).
fn hash_key(key: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

// ──────────────────────────────── peticiones ────────────────────────────────

#[derive(Debug, Clone)]
pub struct HttpError {
    pub status: u16,
    pub url: String,
    pub body: String,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.body.is_empty() {
            write!(f, "HTTP {}", self.status)
        } else {
            write!(f, "HTTP {}: {}", self.status, crate::util::ellipsize(&self.body, 160))
        }
    }
}

impl std::error::Error for HttpError {}

pub struct JsonRequest<'a> {
    url: &'a str,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    timeout: Option<Duration>,
    cache: Option<(&'a DiskCache, Duration)>,
}

impl<'a> JsonRequest<'a> {
    pub fn new(url: &'a str) -> Self {
        Self {
            url,
            query: Vec::new(),
            headers: Vec::new(),
            timeout: None,
            cache: None,
        }
    }

    pub fn query(mut self, k: impl Into<String>, v: impl ToString) -> Self {
        let v = v.to_string();
        if !v.is_empty() {
            self.query.push((k.into(), v));
        }
        self
    }

    pub fn query_all(mut self, it: impl IntoIterator<Item = (String, String)>) -> Self {
        self.query.extend(it);
        self
    }

    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }

    pub fn headers(mut self, it: impl IntoIterator<Item = (String, String)>) -> Self {
        self.headers.extend(it);
        self
    }

    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }

    /// Sirve del caché a disco si tiene menos de `max_age`.
    pub fn cache(mut self, cache: &'a DiskCache, max_age: Duration) -> Self {
        self.cache = Some((cache, max_age));
        self
    }

    pub fn cache_key(&self) -> String {
        let mut key = self.url.to_string();
        for (k, v) in &self.query {
            key.push('|');
            key.push_str(k);
            key.push('=');
            key.push_str(v);
        }
        key
    }

    pub fn bytes(self) -> Result<Vec<u8>> {
        if let Some((cache, max_age)) = self.cache {
            let key = self.cache_key();
            if let Some((body, age)) = cache.get(&key, max_age) {
                tracing::trace!("cache HIT ({}): {}", crate::util::fmt_latency(age), key);
                return Ok(body);
            }
            let res = self.do_request()?;
            cache.put(&key, &res);
            return Ok(res);
        }
        self.do_request()
    }

    pub fn json<T: serde::de::DeserializeOwned>(self) -> Result<T> {
        let url = self.url.to_string();
        let bytes = self.bytes().with_context(|| url.clone())?;
        serde_json::from_slice::<T>(&bytes)
            .map_err(|e| anyhow!("JSON inválido de {url}: {e} ({})", snippet(&bytes)))
    }

    fn do_request(&self) -> Result<Vec<u8>> {
        let host = host_of(self.url);
        let started = Instant::now();

        let mut req = agent().get(self.url);
        for (k, v) in &self.query {
            req = req.query(k, v);
        }
        req = req.header("Accept", "application/json, text/plain, */*");
        req = req.header("Accept-Language", "es-AR,es;q=0.9,en;q=0.8");
        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(t) = self.timeout {
            req = req.config().timeout_global(Some(t)).build();
        }

        let mut resp = match req.call() {
            Ok(r) => r,
            Err(e) => {
                let ms = started.elapsed().as_millis() as u64;
                stats().record(&host, false, 0, ms);
                return Err(map_err(e, self.url));
            }
        };

        let status = resp.status().as_u16();
        let mut buf = Vec::with_capacity(
            resp.headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(8 * 1024)
                .min(MAX_BODY as usize),
        );
        let mut reader = resp.body_mut().with_config().limit(MAX_BODY).reader();
        let read_res = reader.read_to_end(&mut buf);
        let ms = started.elapsed().as_millis() as u64;

        if let Err(e) = read_res {
            stats().record(&host, false, buf.len() as u64, ms);
            return Err(anyhow!("error leyendo respuesta de {}: {e}", self.url));
        }

        stats().record(&host, status < 400, buf.len() as u64, ms);
        tracing::trace!("GET {} -> {} ({} en {}ms)", self.url, status, buf.len(), ms);

        if !(200..300).contains(&status) {
            return Err(HttpError {
                status,
                url: self.url.to_string(),
                body: String::from_utf8_lossy(&buf).trim().to_string(),
            }
            .into());
        }
        Ok(buf)
    }
}

fn snippet(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(b);
    crate::util::ellipsize(s.trim(), 120)
}

fn map_err(e: ureq::Error, url: &str) -> anyhow::Error {
    use ureq::Error;
    match &e {
        Error::StatusCode(c) => HttpError {
            status: *c,
            url: url.to_string(),
            body: String::new(),
        }
        .into(),
        Error::Timeout(_) => anyhow!("timeout en {url}"),
        Error::HostNotFound => anyhow!("no se pudo resolver el host de {url}"),
        Error::Io(io) => anyhow!("error de red en {url}: {io}"),
        _ => anyhow!("{e} ({url})"),
    }
}

pub fn host_of(url: &str) -> String {
    let after = url.split("://").nth(1).unwrap_or(url);
    after.split(['/', '?', '#']).next().unwrap_or(after).to_string()
}

/// GET simple de bytes (imágenes, etc.) con cabeceras opcionales.
pub fn get_bytes(url: &str, headers: &[(String, String)], timeout: Duration) -> Result<Vec<u8>> {
    let host = host_of(url);
    let started = Instant::now();
    let mut req = agent().get(url).config().timeout_global(Some(timeout)).build();
    req = req.header("Accept", "image/*,*/*;q=0.8");
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let mut resp = req.call().map_err(|e| map_err(e, url))?;
    let mut buf = Vec::new();
    resp.body_mut()
        .with_config()
        .limit(48 * 1024 * 1024)
        .reader()
        .read_to_end(&mut buf)
        .with_context(|| format!("leyendo {url}"))?;
    let status = resp.status().as_u16();
    stats().record(
        &host,
        status < 400,
        buf.len() as u64,
        started.elapsed().as_millis() as u64,
    );
    if !(200..300).contains(&status) {
        return Err(HttpError {
            status,
            url: url.to_string(),
            body: String::new(),
        }
        .into());
    }
    Ok(buf)
}

/// Resultado detallado de probar una URL de audio.
#[derive(Debug, Clone, Default)]
pub struct ProbeResult {
    pub status: u16,
    pub content_length: Option<u64>,
    pub content_type: Option<String>,
    pub range_ok: bool,
    /// tamaño total según `Content-Range: bytes 0-1023/12345678`
    pub range_total: Option<u64>,
}

/// Prueba una URL de audio: primero HEAD, y si el servidor lo rechaza, un GET
/// con `Range: bytes=0-1023`. Devuelve estado, tamaño, content-type y si el
/// servidor soporta Range (necesario para el seek).
pub fn probe_verbose(url: &str, headers: &[(String, String)]) -> Result<ProbeResult> {
    let agent = agent();
    let timeout = Duration::from_secs(8);

    let mut head = agent
        .head(url)
        .config()
        .timeout_global(Some(timeout))
        .build();
    for (k, v) in headers {
        head = head.header(k.as_str(), v.as_str());
    }
    if let Ok(r) = head.call() {
        let status = r.status().as_u16();
        if status != 405 && status != 501 && status != 403 && status != 400 {
            return Ok(ProbeResult {
                status,
                content_length: header_u64(&r, "content-length"),
                content_type: header_str(&r, "content-type"),
                range_ok: header_str(&r, "accept-ranges")
                    .map(|v| !v.eq_ignore_ascii_case("none"))
                    .unwrap_or(false),
                range_total: None,
            });
        }
    }

    // HEAD no sirve -> GET parcial
    let mut get = agent
        .get(url)
        .config()
        .timeout_global(Some(timeout))
        .build();
    get = get.header("Range", "bytes=0-1023");
    for (k, v) in headers {
        get = get.header(k.as_str(), v.as_str());
    }
    let r = get.call().map_err(|e| map_err(e, url))?;
    let status = r.status().as_u16();
    let range_total = header_str(&r, "content-range").and_then(|v| {
        v.rsplit('/').next().and_then(|n| n.trim().parse::<u64>().ok())
    });
    Ok(ProbeResult {
        status,
        content_length: header_u64(&r, "content-length"),
        content_type: header_str(&r, "content-type"),
        range_ok: status == 206,
        range_total,
    })
}

fn header_str(r: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    r.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
}

fn header_u64(r: &ureq::http::Response<ureq::Body>, name: &str) -> Option<u64> {
    header_str(r, name).and_then(|s| s.parse().ok())
}

/// HEAD ligero para validar una URL de stream antes de mandarla al reproductor.
pub fn probe(url: &str, headers: &[(String, String)]) -> Result<(u16, Option<u64>, Option<String>)> {
    let mut req = agent().head(url).config().timeout_global(Some(Duration::from_secs(8))).build();
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    // algunos CDN rechazan HEAD -> si falla, se hace GET con Range
    match req.call() {
        Ok(r) => Ok(describe(&r)),
        Err(_) => {
            let mut g = agent()
                .get(url)
                .config()
                .timeout_global(Some(Duration::from_secs(8)))
                .build();
            g = g.header("Range", "bytes=0-1023");
            for (k, v) in headers {
                g = g.header(k.as_str(), v.as_str());
            }
            let r = g.call().map_err(|e| map_err(e, url))?;
            Ok(describe(&r))
        }
    }
}

fn describe(r: &ureq::http::Response<ureq::Body>) -> (u16, Option<u64>, Option<String>) {
    let status = r.status().as_u16();
    let len = r
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let ct = r
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    (status, len, ct)
}

/// Detecta si la conexión actual es "celular"/metered (para settings perNetwork).
pub fn is_metered() -> bool {
    if let Ok(v) = std::env::var("SONIDO_NETWORK") {
        return matches!(v.to_ascii_lowercase().as_str(), "cellular" | "metered" | "mobile");
    }
    let Ok(list) = std::fs::read_dir("/sys/class/net") else {
        return false;
    };
    for e in list.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name == "lo" {
            continue;
        }
        let is_wwan = ["wwan", "rmnet", "usb", "cdc-wdm", "qmi", "ppp"]
            .iter()
            .any(|p| name.starts_with(p));
        if !is_wwan {
            continue;
        }
        let up = std::fs::read_to_string(format!("/sys/class/net/{name}/operstate"))
            .map(|s| s.trim() == "up")
            .unwrap_or(false);
        if up {
            return true;
        }
    }
    false
}

/// Cliente compartible (barato: solo un `Arc` del caché).
#[derive(Clone)]
pub struct Http {
    pub cache: Arc<DiskCache>,
}

impl Http {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            cache: Arc::new(DiskCache::new(cache_dir)),
        }
    }
    pub fn json<'a>(&'a self, url: &'a str) -> JsonRequest<'a> {
        JsonRequest::new(url)
    }
}
