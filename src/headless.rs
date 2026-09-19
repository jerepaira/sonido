//! Modo sin interfaz: instalar addons, buscar, resolver streams y reproducir
//! desde la terminal. Útil para verificar un addon sin abrir la ventana.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};

use crate::addon::Addon;
use crate::config::Store;
use crate::http::Http;
use crate::models::*;
use crate::net::{friendly_err, NetworkKind, Registry};
use crate::player::{Player, PlayerEvent};
use crate::resolve::{ResolvePrefs, Resolver};

pub struct Headless {
    pub store: Arc<Store>,
    pub reg: Arc<Registry>,
    pub resolver: Resolver,
    pub net: NetworkKind,
}

impl Headless {
    pub fn new() -> Self {
        let store = Arc::new(Store::load());
        let http = Http::new(crate::config::cache_dir().join("http"));
        let reg = Arc::new(Registry::new(store.clone(), http));
        Self {
            store,
            reg,
            resolver: Resolver::new(),
            net: NetworkKind::detect(),
        }
    }

    pub fn resolve_prefs(&self) -> ResolvePrefs {
        let p = crate::addon::rg(&self.store.config).prefs.clone();
        ResolvePrefs {
            prefer_hires: p.prefer_hires,
            prefer_atmos: p.prefer_atmos,
            net: self.net,
            probe_stream: p.probe_stream,
        }
    }

    pub fn install(&self, url: &str) -> Result<Addon> {
        let a = self.reg.install(url, true)?;
        println!("✔ {} v{}", a.name(), a.version());
        let m = crate::addon::rg(&a.manifest).clone();
        println!("  id        : {}", m.id);
        if let Some(d) = &m.description {
            println!("  descripción: {}", crate::util::ellipsize(d, 100));
        }
        println!("  resources : {}", m.resources.join(", "));
        println!("  types     : {}", m.types.join(", "));
        println!("  settings  : {}", m.settings.len());
        for s in &m.settings {
            println!(
                "     - {} ({:?}) default={}{}",
                s.key,
                s.kind,
                s.default_str().unwrap_or_else(|| "—".into()),
                if s.per_network { " [perNetwork]" } else { "" }
            );
        }
        if !m.catalogs.is_empty() {
            println!("  catálogos :");
            for c in &m.catalogs {
                println!("     - {} ({}) → {}", c.id, c.r#type, c.name);
            }
        }
        println!("  base      : {}", a.base);
        Ok(a)
    }

    pub fn list(&self) {
        let all = self.reg.all();
        if all.is_empty() {
            println!("(no hay addons instalados)");
            return;
        }
        for a in all {
            let m = crate::addon::rg(&a.manifest).clone();
            println!(
                "{} {} v{} · prio {} · {} · resources: {}",
                if a.enabled() { "✔" } else { "✘" },
                a.name(),
                m.version,
                a.priority(),
                a.base,
                m.resources.join(",")
            );
        }
    }

    pub fn remove(&self, id_or_url: &str) -> Result<()> {
        let base = if id_or_url.contains("://") || id_or_url.contains('/') {
            crate::util::normalize_addon_base(id_or_url)?
        } else {
            self.reg
                .by_id(id_or_url)
                .map(|a| a.base.clone())
                .ok_or_else(|| anyhow::anyhow!("no existe el addon {id_or_url}"))?
        };
        self.reg.remove(&base);
        println!("✔ desinstalado {base}");
        Ok(())
    }

    pub fn set_setting(&self, addon: &str, key: &str, value: &str, cellular: bool) -> Result<()> {
        let a = self
            .reg
            .by_id(addon)
            .ok_or_else(|| anyhow::anyhow!("no existe el addon {addon}"))?;
        let net = if cellular {
            NetworkKind::Cellular
        } else {
            NetworkKind::Wifi
        };
        self.reg.set_setting(&a.base, key, value.to_string(), net);
        println!("✔ {addon} · {key} = {value:?} ({})", net.label());
        Ok(())
    }

    pub fn search(&self, addon: Option<&str>, q: &str, limit: usize) -> Result<()> {
        let addons: Vec<Addon> = match addon {
            Some(id) => vec![self
                .reg
                .by_id(id)
                .ok_or_else(|| anyhow::anyhow!("no existe el addon {id}"))?],
            None => self.reg.enabled(),
        };
        if addons.is_empty() {
            bail!("no hay addons habilitados");
        }

        // todas las peticiones en paralelo, cada una en su hilo
        let mut handles = Vec::new();
        for a in addons {
            let reg = self.reg.clone();
            let net = self.net;
            let q = q.to_string();
            handles.push(std::thread::spawn(move || {
                let t = std::time::Instant::now();
                let id = a.id();
                let name = a.name();
                (
                    id,
                    name,
                    reg.api.search(&a, &q, net),
                    t.elapsed(),
                )
            }));
        }

        for h in handles {
            let (id, name, res, elapsed) = h.join().unwrap_or_else(|_| {
                (
                    "?".into(),
                    "?".into(),
                    Err(anyhow::anyhow!("panic en el worker")),
                    Duration::ZERO,
                )
            });
            match res {
                Ok(mut r) => {
                    crate::addon::apply_explicit_filter(&mut r, false);
                    println!(
                        "\n── {name} ({id}) · {} en {} ──",
                        r.total(),
                        crate::util::fmt_latency(elapsed)
                    );
                    print_tracks(&r.tracks, limit, &id);
                    print_albums(&r.albums, limit.min(10));
                    print_artists(&r.artists, limit.min(10));
                    print_playlists(&r.playlists, limit.min(10));
                }
                Err(e) => println!("\n── {name} ({id}) · ERROR: {} ──", friendly_err(&e)),
            }
        }
        Ok(())
    }

    /// Vuelca el JSON **crudo** de cualquier endpoint del addon.
    pub fn dump(&self, addon: Option<&str>, path: &str, query: &[(&str, String)]) -> Result<()> {
        let a = self.pick(addon)?;
        let t = std::time::Instant::now();
        let raw = self.reg.api.fetch_raw(&a, path, query, self.net)?;
        println!("{raw}");
        eprintln!("\n── {} en {}", a.name(), crate::util::fmt_latency(t.elapsed()));
        Ok(())
    }

    fn pick(&self, addon: Option<&str>) -> Result<Addon> {
        match addon {
            Some(id) => self
                .reg
                .by_id(id)
                .ok_or_else(|| anyhow::anyhow!("no existe el addon {id} (mirá `sonido list`)")),
            None => self
                .reg
                .enabled()
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no hay addons habilitados")),
        }
    }

    /// Diagnóstico completo de la cadena de reproducción de un addon.
    pub fn doctor(&self, addon: Option<&str>, query: &str) -> Result<()> {
        let a = self.pick(addon)?;
        let aid = a.id();
        let step = |n: &str| println!("\n\x1b[1m── {n} ──\x1b[0m");

        step("1. manifest");
        match self.reg.api.fetch_manifest(&a.base, Duration::ZERO) {
            Ok(m) => {
                println!("  ok · {} v{}", m.display_name(), m.version);
                println!("  resources : {}", m.resources.join(", "));
                println!("  types     : {}", m.types.join(", "));
                println!("  catalogs  : {}", m.catalogs.len());
                println!("  settings  : {}", m.settings.len());
            }
            Err(e) => println!("  \x1b[31mFALLÓ\x1b[0m: {}", friendly_err(&e)),
        }

        step("2. settings que se están mandando (query params)");
        for (k, v) in a.settings_params(self.net) {
            println!("  {k} = {v:?}");
        }
        let hdrs = crate::addon::rg(&a.entry).headers.clone();
        if hdrs.is_empty() {
            println!("  (sin headers extra)");
        } else {
            for (k, v) in &hdrs {
                println!("  header {k}: {}", crate::util::ellipsize(v, 40));
            }
        }
        println!("  red detectada: {}", self.net.label());

        step(&format!("3. search?q={query:?}  (JSON crudo)"));
        let t = std::time::Instant::now();
        let raw = self
            .reg
            .api
            .fetch_raw(&a, "search", &[("q", query.to_string())], self.net);
        println!("  latencia: {}", crate::util::fmt_latency(t.elapsed()));
        let raw = match raw {
            Ok(r) => r,
            Err(e) => {
                println!("  \x1b[31mFALLÓ\x1b[0m: {}", friendly_err(&e));
                return Ok(());
            }
        };
        println!("{}", indent(&raw, "  "));

        let parsed: SearchResponse = match serde_json::from_str::<SearchResponse>(&raw) {
            Ok(p) => p.normalized(),
            Err(e) => {
                println!("  \x1b[31mno es JSON válido\x1b[0m: {e}");
                return Ok(());
            }
        };
        println!(
            "  → {} temas, {} álbumes, {} artistas, {} playlists",
            parsed.tracks.len(),
            parsed.albums.len(),
            parsed.artists.len(),
            parsed.playlists.len()
        );
        let Some(track) = parsed.tracks.first() else {
            println!("  sin temas para probar el stream");
            return Ok(());
        };
        println!(
            "  primer tema: {} — {}  id={}  isrc={:?}  artwork={:?}",
            track.title,
            track.artist,
            track.id,
            track.isrc,
            track.artwork_url.as_deref().map(|u| crate::util::ellipsize(u, 60))
        );

        step(&format!("4. stream/{}  (JSON crudo)", track.id));
        let t = std::time::Instant::now();
        let raw = self.reg.api.fetch_raw(
            &a,
            &format!("stream/{}", track.id),
            &[],
            self.net,
        )?;
        println!("  latencia: {}", crate::util::fmt_latency(t.elapsed()));
        println!("{}", indent(&raw, "  "));

        step("5. interpretación de la respuesta");
        let s: StreamResponse = serde_json::from_str(&raw).unwrap_or_default();
        match &s.url {
            Some(u) => println!("  url       : {}", u),
            None => {
                println!("  \x1b[31mno hay campo `url`\x1b[0m — el addon no devolvió audio");
                println!("  campos presentes: {:?}", s.extra.keys().collect::<Vec<_>>());
                return Ok(());
            }
        }
        println!("  codec     : {:?} · contenedor {:?} · manifest {:?}", s.codec, s.container, s.manifest);
        println!(
            "  audio     : {:?}-bit / {:?} Hz · {:?} ch · cifrado={:?}",
            s.bit_depth, s.sample_rate, s.channels, s.encrypted
        );
        println!("  headers   : {:?}", s.headers());
        println!("  opciones  : {:?}", s.player_options());
        if let Some(exp) = s.expires_at {
            let now = crate::config::now_secs();
            let left = exp as i64 - now as i64;
            println!(
                "  expira    : {exp} ({})",
                if left <= 0 {
                    format!("\x1b[31mvencida hace {} s\x1b[0m", -left)
                } else {
                    format!("quedan {left} s")
                }
            );
        }
        if let Some(v) = &s.video {
            println!("  vídeo     : {:?} muxed={:?}", v.url, v.muxed);
        }

        step("6. verificación de la URL de audio (HEAD / Range)");
        let url = s.url.clone().unwrap_or_default();
        let headers = s.headers();
        match crate::resolve::verify_stream(&url, &headers) {
            crate::resolve::Probe::Ok { status, bytes, content_type, range_ok, elapsed } => {
                println!("  \x1b[32mHTTP {status}\x1b[0m en {}", crate::util::fmt_latency(elapsed));
                println!(
                    "  tamaño    : {}",
                    bytes.map(crate::util::fmt_bytes).unwrap_or_else(|| "?".into())
                );
                println!("  tipo      : {:?}", content_type);
                println!("  Range/seek: {}", if range_ok { "soportado" } else { "NO soportado (el seek puede fallar)" });
            }
            crate::resolve::Probe::Warn(w) => println!("  \x1b[33maviso\x1b[0m: {w}"),
            crate::resolve::Probe::Fatal(f) => println!("  \x1b[31mNO REPRODUCIBLE\x1b[0m: {f}"),
            crate::resolve::Probe::Skipped => println!("  (sin verificar)"),
        }

        step("7. portada del primer tema");
        match track.artwork_url.as_deref() {
            Some(u) => {
                println!("  url: {u}");
                match crate::http::get_bytes(u, &[], Duration::from_secs(12)) {
                    Ok(b) => {
                        println!("  \x1b[32mok\x1b[0m · {}", crate::util::fmt_bytes(b.len() as u64));
                        match image::guess_format(&b) {
                            Ok(f) => println!("  formato: {f:?}"),
                            Err(e) => println!("  \x1b[33mno es una imagen reconocible\x1b[0m: {e}"),
                        }
                    }
                    Err(e) => println!("  \x1b[31mFALLÓ\x1b[0m: {}", friendly_err(&e)),
                }
            }
            None => println!(
                "  \x1b[33mel tema no trae portada\x1b[0m (ningún campo de los que conoce la app: {:?})",
                crate::models::ARTWORK_ALIASES
            ),
        }

        step("8. resumen");
        println!("  addon      : {aid}");
        println!("  base       : {}", a.base);
        println!("  motor      : ver `sonido info`");
        println!("  Si el paso 4 devuelve una URL de un host local (127.0.0.1/localhost),");
        println!("  el addon necesita que corras su servicio auxiliar en tu máquina.");
        println!("  Si el paso 6 da 403/404, la URL expira: es el addon el que debe");
        println!("  regenerarla, no la app.");
        Ok(())
    }

    pub fn stream(&self, addon: Option<&str>, track_id: &str) -> Result<()> {
        let a = match addon {
            Some(id) => self
                .reg
                .by_id(id)
                .ok_or_else(|| anyhow::anyhow!("no existe el addon {id}"))?,
            None => self
                .reg
                .enabled()
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no hay addons habilitados"))?,
        };
        let t = std::time::Instant::now();
        let track = Track {
            id: track_id.to_string(),
            ..Default::default()
        };
        let mut r = self.resolver.resolve_direct(&self.reg.api, &a, &track, &self.resolve_prefs())?;
        r.probe = crate::resolve::verify_stream(&r.url, &r.headers);
        println!("addon     : {} ({})", r.addon_name, r.addon_id);
        println!("tipo      : {}", r.adaptive.label());
        println!("url       : {}", r.url);
        if let Some(i) = &r.isrc {
            println!("isrc      : {i}");
        }
        println!("codec     : {:?}", r.stream.codec);
        println!("contenedor: {:?}", r.stream.container);
        println!("manifest  : {:?}", r.stream.manifest);
        println!("calidad   : {:?}", r.stream.quality);
        if let (Some(b), Some(sr)) = (r.stream.bit_depth, r.stream.sample_rate) {
            println!("audio     : {b}-bit / {sr} Hz");
        }
        if let Some(br) = r.stream.bitrate {
            println!("bitrate   : {br} bps");
        }
        if r.stream.is_atmos() {
            println!("atmos     : sí");
        }
        if let Some(exp) = r.stream.expires_at {
            println!("expira    : {exp} (unix)");
        }
        if !r.stream.headers().is_empty() {
            println!("headers   : {:?}", r.stream.headers());
        }
        if let Some(v) = &r.stream.video {
            println!("vídeo     : {:?} muxed={:?}", v.url, v.muxed);
            for r in &v.renditions {
                println!("   - {}p {}", r.height.unwrap_or(0), r.url);
            }
        }
        if !r.stream.chapters.is_empty() {
            println!("capítulos : {}", r.stream.chapters.len());
            for c in r.stream.chapters.iter().take(10) {
                println!(
                    "   - {} @ {}",
                    c.title,
                    crate::util::fmt_duration(c.start_time.unwrap_or(0.0))
                );
            }
        }
        println!("verificación : {}", r.probe.label());
        if r.adaptive != crate::resolve::Adaptive::None {
            println!(
                "nota      : es streaming adaptativo; se reproduce con libmpv (demuxer={})",
                if r.adaptive == crate::resolve::Adaptive::Dash { "dash" } else { "hls" }
            );
        }
        println!("resuelto en {}", crate::util::fmt_latency(t.elapsed()));
        Ok(())
    }

    /// Reproduce por consola (Ctrl-C para salir).
    pub fn play(&self, addon: Option<&str>, track_id: &str) -> Result<()> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let _guard = rt.enter();

        let a = match addon {
            Some(id) => self
                .reg
                .by_id(id)
                .ok_or_else(|| anyhow::anyhow!("no existe el addon {id}"))?,
            None => self
                .reg
                .enabled()
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("no hay addons habilitados"))?,
        };
        let track = Track {
            id: track_id.to_string(),
            ..Default::default()
        };
        println!("resolviendo stream…");
        let t = std::time::Instant::now();
        let r = self.resolver.resolve_direct(&self.reg.api, &a, &track, &self.resolve_prefs())?;
        println!("ok en {} → {}", crate::util::fmt_latency(t.elapsed()), r.url);

        let player = Player::create();
        println!("motor: {}", player.backend_name());
        let item = crate::player::QueueItem::new(&a, &track);
        player.play_resolved(&item, &r, 0.0);

        println!("reproduciendo (Ctrl-C para terminar)…");
        let mut last = 0i64;
        loop {
            std::thread::sleep(Duration::from_millis(200));
            for e in player.drain_events() {
                match e {
                    PlayerEvent::Failed { error, .. } => {
                        println!("\n✘ {error}");
                        return Ok(());
                    }
                    PlayerEvent::Idle => {
                        println!("\n✔ terminado");
                        return Ok(());
                    }
                    PlayerEvent::Duration(d) => {
                        println!("duración: {}", crate::util::fmt_duration(d));
                    }
                    PlayerEvent::AudioInfo(i) => println!("audio: {}", i.badge()),
                    _ => {}
                }
            }
            let pos = player.position() as i64;
            if pos != last {
                last = pos;
                print!(
                    "\r{} / {}   ",
                    crate::util::fmt_duration(pos as f64),
                    crate::util::fmt_duration(player.duration())
                );
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
        }
    }

    /// Descarga una fila de catálogo.
    pub fn catalog(&self, addon: &str, catalog_id: &str, skip: u32) -> Result<()> {
        let a = self
            .reg
            .by_id(addon)
            .ok_or_else(|| anyhow::anyhow!("no existe el addon {addon}"))?;
        let t = std::time::Instant::now();
        let r = self
            .reg
            .api
            .catalog(&a, catalog_id, skip, self.net, Duration::ZERO)?;
        println!(
            "{} ítems en {}",
            r.items.len(),
            crate::util::fmt_latency(t.elapsed())
        );
        for (i, it) in r.items.iter().enumerate() {
            println!(
                "{:>3}. [{}] {} — {}  {}{}",
                i + 1,
                it.r#type,
                crate::util::ellipsize(&it.title, 46),
                crate::util::ellipsize(&it.artist, 30),
                crate::util::fmt_duration_opt(it.duration_secs()),
                it.isrc.as_deref().map(|s| format!("  {s}")).unwrap_or_default()
            );
        }
        Ok(())
    }

    pub fn detail(&self, addon: &str, kind: &str, id: &str) -> Result<()> {
        let a = self
            .reg
            .by_id(addon)
            .ok_or_else(|| anyhow::anyhow!("no existe el addon {addon}"))?;
        let t = std::time::Instant::now();
        match kind {
            "album" => {
                let al = self.reg.api.album(&a, id, self.net, Duration::ZERO)?;
                println!("{} — {} ({})", al.title, al.artist, al.year_str());
                print_tracks(&al.tracks, 500, addon);
            }
            "artist" => {
                let ar = self.reg.api.artist(&a, id, self.net, Duration::ZERO)?;
                println!("{} · géneros: {}", ar.name, ar.genres.join(", "));
                println!("top tracks:");
                print_tracks(&ar.top_tracks, 20, addon);
                println!("álbumes: {}", ar.albums.len());
                for al in ar.albums.iter().take(30) {
                    println!("   - {} ({})", al.title, al.year_str());
                }
            }
            "playlist" => {
                let pl = self.reg.api.playlist(&a, id, self.net, Duration::ZERO)?;
                println!("{} · por {}", pl.title, pl.creator.clone().unwrap_or_default());
                print_tracks(&pl.tracks, 500, addon);
            }
            other => bail!("tipo desconocido: {other} (album|artist|playlist)"),
        }
        println!("\n({})", crate::util::fmt_latency(t.elapsed()));
        Ok(())
    }
}

fn indent(s: &str, pad: &str) -> String {
    s.lines()
        .map(|l| format!("{pad}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn print_tracks(tracks: &[Track], limit: usize, addon_id: &str) {
    if tracks.is_empty() {
        return;
    }
    println!("  temas ({}):", tracks.len().min(limit));
    for (i, t) in tracks.iter().take(limit).enumerate() {
        println!(
            "  {:>3}. {} — {}  [{}] {}{}{}",
            i + 1,
            crate::util::ellipsize(&t.title, 46),
            crate::util::ellipsize(&t.artist, 30),
            t.id,
            crate::util::fmt_duration_opt(t.duration_secs()),
            t.isrc
                .as_deref()
                .map(|s| format!("  {s}"))
                .unwrap_or_default(),
            if t.stream_url.is_some() { "  (streamURL)" } else { "" },
        );
    }
    let _ = addon_id;
}

fn print_albums(items: &[Album], limit: usize) {
    if items.is_empty() {
        return;
    }
    println!("  álbumes ({}):", items.len().min(limit));
    for (i, a) in items.iter().take(limit).enumerate() {
        println!(
            "  {:>3}. {} — {} ({}) [{}]",
            i + 1,
            crate::util::ellipsize(&a.title, 46),
            crate::util::ellipsize(&a.artist, 30),
            a.year_str(),
            a.id
        );
    }
}

fn print_artists(items: &[Artist], limit: usize) {
    if items.is_empty() {
        return;
    }
    println!("  artistas ({}):", items.len().min(limit));
    for (i, a) in items.iter().take(limit).enumerate() {
        println!(
            "  {:>3}. {} [{}] {}",
            i + 1,
            crate::util::ellipsize(&a.name, 40),
            a.id,
            a.genres.iter().take(3).cloned().collect::<Vec<_>>().join(",")
        );
    }
}

fn print_playlists(items: &[Playlist], limit: usize) {
    if items.is_empty() {
        return;
    }
    println!("  playlists ({}):", items.len().min(limit));
    for (i, p) in items.iter().take(limit).enumerate() {
        println!(
            "  {:>3}. {} [{}] {}",
            i + 1,
            crate::util::ellipsize(&p.title, 46),
            p.id,
            p.creator.clone().unwrap_or_default()
        );
    }
}
