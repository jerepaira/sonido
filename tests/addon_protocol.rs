//! Pruebas de integración contra el addon mock (`tools/mock_addon.py`).
//!
//! Verifican el protocolo completo: instalación, búsqueda, settings como query
//! params, catálogos, detalle, resolución de streams, caché y cadena de fallback.
//!
//! Corren en serie (comparten el puerto) y requieren `python3` en el PATH.

use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use sonido::addon::{AddonApi, TrackIdentity};
use sonido::config::{ConfigFile, Store};
use sonido::http::Http;
use sonido::models::*;
use sonido::net::{NetworkKind, Registry};
use sonido::player::Repeat;
use sonido::resolve::{ResolvePrefs, Resolver};

const PORT: u16 = 8799;

fn serial() -> &'static Mutex<()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

struct Mock {
    child: Child,
    base: String,
}

impl Mock {
    fn start() -> Option<Self> {
        let root = env!("CARGO_MANIFEST_DIR");
        let child = Command::new("python3")
            .arg(format!("{root}/tools/mock_addon.py"))
            .args(["--port", &PORT.to_string(), "--seconds", "3"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let base = format!("http://127.0.0.1:{PORT}");
        // esperar a que levante
        for _ in 0..60 {
            if let Ok(m) = AddonApi::new(Http::new(std::env::temp_dir().join(format!(
                "sonido-test-{}",
                std::process::id()
            ))))
            .fetch_manifest(&base, Duration::ZERO)
            {
                if !m.id.is_empty() {
                    return Some(Self { child, base });
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        None
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Ctx {
    base: String,
    reg: std::sync::Arc<Registry>,
    resolver: Resolver,
    tmp: std::path::PathBuf,
}

impl Ctx {
    fn new(base: &str) -> Self {
        let tmp = std::env::temp_dir().join(format!("sonido-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("HOME", &tmp);
        std::env::set_var("XDG_CONFIG_HOME", tmp.join("config"));
        std::env::set_var("XDG_CACHE_HOME", tmp.join("cache"));
        std::env::set_var("XDG_DATA_HOME", tmp.join("data"));

        let store = std::sync::Arc::new(Store::load());
        let http = Http::new(tmp.join("cache").join("http"));
        let reg = std::sync::Arc::new(Registry::new(store, http));
        Self {
            base: base.to_string(),
            reg,
            resolver: Resolver::new(),
            tmp,
        }
    }

    fn prefs(&self) -> ResolvePrefs {
        ResolvePrefs {
            prefer_hires: true,
            prefer_atmos: false,
            net: NetworkKind::Wifi,
            probe_stream: true,
        }
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

#[test]
fn protocolo_completo() {
    let _g = serial().lock().unwrap_or_else(|e| e.into_inner());
    let Some(mock) = Mock::start() else {
        eprintln!("no se pudo arrancar tools/mock_addon.py (¿falta python3?) — test salteado");
        return;
    };
    let ctx = Ctx::new(&mock.base);

    // ── 1. instalación ──
    let addon = ctx.reg.install(&mock.base, true).expect("install");
    assert_eq!(addon.id(), "com.sonido.mock");
    assert!(addon.has("search") && addon.has("stream") && addon.has("catalog"));
    assert_eq!(addon.settings_defs().len(), 4);
    assert_eq!(addon.catalogs().len(), 3);

    // ── 2. búsqueda ──
    let res = ctx
        .reg
        .api
        .search(&addon, "luna", NetworkKind::Wifi)
        .expect("search");
    assert!(!res.tracks.is_empty(), "la búsqueda no devolvió temas");
    let t = &res.tracks[0];
    assert_eq!(t.title, "Luna sobre el río");
    assert_eq!(t.isrc.as_deref(), Some("ARMO1234500001"));
    assert!(t.duration_secs().unwrap() > 100.0, "duration en segundos");

    // ── 3. settings como query params ──
    ctx.reg
        .set_setting(&addon.base, "quality", "LOSSLESS".into(), NetworkKind::Wifi);
    ctx.reg
        .set_setting(&addon.base, "quality", "LOW".into(), NetworkKind::Cellular);
    let addon = ctx.reg.by_id("com.sonido.mock").unwrap();
    let params = addon.settings_params(NetworkKind::Wifi);
    assert!(
        params.iter().any(|(k, v)| k == "quality" && v == "LOSSLESS"),
        "settings Wi-Fi: {params:?}"
    );
    let params_c = addon.settings_params(NetworkKind::Cellular);
    assert!(
        params_c.iter().any(|(k, v)| k == "quality" && v == "LOW"),
        "settings celular: {params_c:?}"
    );

    let s = ctx
        .resolver
        .resolve_direct(
            &ctx.reg.api,
            &addon,
            &Track {
                id: "t7".into(),
                ..Default::default()
            },
            &ctx.prefs(),
        )
        .expect("stream");
    assert!(
        s.url.contains("quality=LOSSLESS"),
        "el setting debe viajar en la URL: {}",
        s.url
    );
    assert_eq!(s.stream.codec.as_deref(), Some("pcm_s16le"));
    assert_eq!(s.stream.bit_depth, Some(16));
    assert_eq!(s.stream.sample_rate, Some(44100));
    assert_eq!(s.stream.chapters.len(), 3);
    assert_eq!(s.via, sonido::resolve::Via::Direct);

    // ── 4. caché de streams ──
    let t0 = std::time::Instant::now();
    let s2 = ctx
        .resolver
        .resolve_direct(
            &ctx.reg.api,
            &addon,
            &Track {
                id: "t7".into(),
                ..Default::default()
            },
            &ctx.prefs(),
        )
        .expect("stream cacheado");
    assert_eq!(s2.via, sonido::resolve::Via::Cache);
    assert_eq!(s2.url, s.url);
    assert!(t0.elapsed() < Duration::from_millis(50), "el caché debe ser instantáneo");

    // ── 5. streamURL directo (cero round-trips) ──
    let album = ctx
        .reg
        .api
        .album(&addon, "al1", NetworkKind::Wifi, Duration::ZERO)
        .expect("album");
    assert_eq!(album.tracks.len(), 5);
    assert!(album.tracks[0].stream_url.is_some(), "el detalle trae streamURL");
    let direct = ctx
        .resolver
        .resolve_direct(&ctx.reg.api, &addon, &album.tracks[0], &ctx.prefs())
        .expect("direct");
    assert_eq!(direct.via, sonido::resolve::Via::StreamUrl);

    // ── 6. resolve-isrc ──
    let id = ctx
        .reg
        .api
        .resolve_isrc(&addon, "ARMO1234500003", NetworkKind::Wifi)
        .expect("isrc");
    assert_eq!(id.as_deref(), Some("t3"));
    let none = ctx
        .reg
        .api
        .resolve_isrc(&addon, "XX0000000000", NetworkKind::Wifi)
        .expect("isrc 404");
    assert!(none.is_none());

    // ── 7. cadena de fallback por identidad ──
    let r = ctx
        .resolver
        .resolve_identity(
            &ctx.reg.api,
            &[addon.clone()],
            &TrackIdentity {
                title: "Bajamar".into(),
                artist: "Litoral Eléctrico".into(),
                isrc: Some("ARMO1234500003".into()),
                duration: Some(232.0),
            },
            &ctx.prefs(),
        )
        .expect("fallback por ISRC");
    assert_eq!(r.track_id, "t3");
    assert_eq!(r.via, sonido::resolve::Via::Isrc);

    // sin ISRC: tiene que resolver por /resolve o por búsqueda puntuada
    let r2 = ctx
        .resolver
        .resolve_identity(
            &ctx.reg.api,
            &[addon.clone()],
            &TrackIdentity {
                title: "Neón".into(),
                artist: "Nadia Sol".into(),
                isrc: None,
                duration: Some(176.0),
            },
            &ctx.prefs(),
        )
        .expect("fallback por identidad");
    assert_eq!(r2.track_id, "t6");
    assert!(
        matches!(r2.via, sonido::resolve::Via::Resolve | sonido::resolve::Via::SearchMatch),
        "vía inesperada: {:?}",
        r2.via
    );

    // un tema que NO existe no debe devolver cualquier cosa
    let bad = ctx.resolver.resolve_identity(
        &ctx.reg.api,
        &[addon.clone()],
        &TrackIdentity {
            title: "Canción inexistente XYZ".into(),
            artist: "Nadie".into(),
            isrc: None,
            duration: None,
        },
        &ctx.prefs(),
    );
    assert!(
        bad.is_err() || bad.as_ref().map(|r| r.track_id.is_empty()).unwrap_or(true),
        "no debe inventar un tema: {bad:?}"
    );

    // ── 8. catálogos + paginado ──
    let cat = ctx
        .reg
        .api
        .catalog(&addon, "top", 0, NetworkKind::Wifi, Duration::ZERO)
        .expect("catalog");
    assert_eq!(cat.items.len(), 16);
    assert_eq!(cat.items[0].kind(), ItemKind::Track);
    assert!(cat.items[0].duration_ms.unwrap() > 100_000, "durationMs en ms");
    assert!(cat.items[0].isrc.is_some());

    // ── 9. artista y playlist ──
    let artist = ctx
        .reg
        .api
        .artist(&addon, "a1", NetworkKind::Wifi, Duration::ZERO)
        .expect("artist");
    assert_eq!(artist.name, "Litoral Eléctrico");
    assert!(!artist.top_tracks.is_empty());
    assert!(!artist.albums.is_empty());

    let pl = ctx
        .reg
        .api
        .playlist(&addon, "p1", NetworkKind::Wifi, Duration::ZERO)
        .expect("playlist");
    assert_eq!(pl.tracks.len(), 4);

    // ── 10. persistencia ──
    ctx.reg.store.save();
    let cfg_path = ctx.reg.store.config_path().clone();
    let raw = std::fs::read_to_string(&cfg_path).expect("addons.json");
    let cfg: ConfigFile = serde_json::from_str(&raw).expect("releer config");
    assert_eq!(cfg.addons.len(), 1);
    assert_eq!(cfg.addons[0].settings.get("quality").map(|s| s.as_str()), Some("LOSSLESS"));
    assert_eq!(
        cfg.addons[0].settings_cellular.get("quality").map(|s| s.as_str()),
        Some("LOW")
    );
    assert!(cfg.addons[0].manifest_cache.is_some(), "el manifest queda cacheado");

    // ── 11. cola ──
    let q = sonido::player::Queue::new();
    let items: Vec<sonido::player::QueueItem> = cat
        .items
        .iter()
        .map(|c| sonido::player::QueueItem::from_catalog(&addon, c))
        .collect();
    q.set(items, 2);
    assert_eq!(q.len(), 16);
    assert_eq!(q.current().unwrap().title, "Bajamar");
    assert!(q.advance());
    assert_eq!(q.cursor(), 3);
    assert!(q.rewind());
    q.set_shuffle(true);
    assert_eq!(q.current().unwrap().title, "Bajamar", "el shuffle conserva el tema actual al frente");
    q.set_repeat(Repeat::All);
    for _ in 0..20 {
        assert!(q.advance());
    }
}

#[test]
fn instalacion_reintentable_y_normalizacion_de_urls() {
    let _g = serial().lock().unwrap_or_else(|e| e.into_inner());
    let Some(mock) = Mock::start() else { return };
    let ctx = Ctx::new(&mock.base);

    for url in [
        format!("{}/manifest.json", mock.base),
        format!("{}/", mock.base),
        mock.base.clone(),
    ] {
        let a = ctx.reg.install(&url, true).unwrap_or_else(|_| panic!("install {url}"));
        assert_eq!(a.base, mock.base);
        assert_eq!(ctx.reg.count(), 1, "no debe duplicar el addon");
    }

    assert!(sonido::util::normalize_addon_base("https://h").is_err());
    assert!(sonido::util::normalize_addon_base("").is_err());

    ctx.reg.remove(&mock.base);
    assert_eq!(ctx.reg.count(), 0);
}
