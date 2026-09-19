//! Estado de la aplicación y cableado UI ⇄ workers.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use egui::Context;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::addon::Addon;
use crate::config::{ConfigFile, Library, Prefs, Store};
use crate::http::Http;
use crate::models::{LibraryTrack, LocalPlaylist};
use crate::images::NetImageLoader;
use crate::models::*;
use crate::net::{self, AddonResult, NetworkKind, Registry};
use crate::player::{AudioInfo, Player, PlayerEvent, QueueItem, Repeat, Slot};
use crate::resolve::{ResolvePrefs, ResolvedStream, Resolver};
use crate::ui::widgets::RowAction;

// ──────────────────────────────── eventos ────────────────────────────────

#[derive(Debug, Clone)]
pub enum AppEvent {
    Player(PlayerEvent),
    /// Stream resuelto listo para sonar.
    PlayReady {
        gen: u64,
        resolved: ResolvedStream,
        start_at: f64,
    },
    PlayFailed {
        gen: u64,
        error: String,
        item: QueueItem,
    },
    PrerollReady {
        gen: u64,
        resolved: ResolvedStream,
    },
    Toast {
        msg: String,
        error: bool,
    },
}

// ──────────────────────────────── servicios ────────────────────────────────

/// Todo lo compartible entre la UI y los workers.
pub struct Services {
    pub rt: Arc<tokio::runtime::Runtime>,
    pub http: Http,
    pub store: Arc<Store>,
    pub reg: Arc<Registry>,
    pub player: Arc<Player>,
    pub resolver: Arc<Resolver>,
    /// Canal principal (reproductor + playback).
    pub tx: UnboundedSender<AppEvent>,
    rx: Mutex<UnboundedReceiver<AppEvent>>,
    /// Canal de resultados de addon (lo consumen `net::spawn_*`).
    pub atx: UnboundedSender<AddonResult>,
    arx: Mutex<UnboundedReceiver<AddonResult>>,
    pub image_loader: Arc<NetImageLoader>,
    /// Portadas resueltas por ISRC para temas que el addon mandó sin artwork.
    /// Clave: `"{addon_id}::{track_id}"`.
    pub artwork: Arc<std::sync::RwLock<std::collections::BTreeMap<String, String>>>,
    pub started: Instant,
}

impl Services {
    pub fn new(ctx: &Context) -> Result<Arc<Self>> {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 6))
            .unwrap_or(4);
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(workers)
                .max_blocking_threads(32)
                .thread_name("sonido-io")
                .enable_time()
                .build()?,
        );

        let cache_root = crate::config::cache_dir();
        let _ = std::fs::create_dir_all(&cache_root);
        let http = Http::new(cache_root.join("http"));
        crate::http::warmup();

        let store = Arc::new(Store::load());
        let reg = Arc::new(Registry::new(store.clone(), http.clone()));
        let player = Arc::new(Player::create());
        let resolver = Arc::new(Resolver::new());

        let (tx, rx) = unbounded_channel::<AppEvent>();
        let (atx, arx) = unbounded_channel::<AddonResult>();

        let image_loader = Arc::new(NetImageLoader::new(rt.handle().clone(), http.cache.clone()));
        {
            let loaders = ctx.loaders();
            let mut guard = loaders.image.lock();
            guard.push(image_loader.clone());
        }

        // Bombea los eventos del backend de audio al canal de la app.
        {
            let tx2 = tx.clone();
            let pl = player.clone();
            std::thread::Builder::new()
                .name("event-pump".into())
                .spawn(move || loop {
                    let evs = pl.drain_events();
                    if evs.is_empty() {
                        std::thread::sleep(Duration::from_millis(12));
                        continue;
                    }
                    for e in evs {
                        if tx2.send(AppEvent::Player(e)).is_err() {
                            return;
                        }
                    }
                })?;
        }

        Ok(Arc::new(Self {
            rt,
            http,
            store,
            reg,
            player,
            resolver,
            tx,
            rx: Mutex::new(rx),
            atx,
            arx: Mutex::new(arx),
            image_loader,
            artwork: Arc::new(std::sync::RwLock::new(std::collections::BTreeMap::new())),
            started: Instant::now(),
        }))
    }

    /// Drena ambos canales. Devuelve `true` si llegó algo.
    pub fn drain(&self) -> (Vec<AppEvent>, Vec<AddonResult>) {
        let mut a = Vec::new();
        let mut b = Vec::new();
        if let Ok(mut rx) = self.rx.lock() {
            while let Ok(e) = rx.try_recv() {
                a.push(e);
            }
        }
        if let Ok(mut rx) = self.arx.lock() {
            while let Ok(e) = rx.try_recv() {
                b.push(e);
            }
        }
        (a, b)
    }

    pub fn prefs(&self) -> Prefs {
        crate::addon::rg(&self.store.config).prefs.clone()
    }

    pub fn set_prefs(&self, f: impl FnOnce(&mut Prefs)) {
        {
            let mut cfg = crate::addon::wg(&self.store.config);
            f(&mut cfg.prefs);
        }
        self.store.mark_config_dirty();
    }

    pub fn library(&self) -> Library {
        crate::addon::rg(&self.store.library).clone()
    }

    pub fn mutate_library(&self, f: impl FnOnce(&mut Library)) {
        {
            let mut l = crate::addon::wg(&self.store.library);
            f(&mut l);
        }
        self.store.mark_library_dirty();
    }

    pub fn config(&self) -> ConfigFile {
        crate::addon::rg(&self.store.config).clone()
    }

    pub fn net(&self) -> NetworkKind {
        NetworkKind::detect()
    }

    pub fn resolve_prefs(&self) -> ResolvePrefs {
        let p = self.prefs();
        ResolvePrefs {
            prefer_hires: p.prefer_hires,
            prefer_atmos: p.prefer_atmos,
            net: self.net(),
            probe_stream: p.probe_stream,
        }
    }

    pub fn spawn<F: FnOnce() + Send + 'static>(&self, f: F) {
        self.rt.spawn(async move {
            tokio::task::spawn_blocking(f).await.ok();
        });
    }
}

// ──────────────────────────────── estado UI ────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    Home,
    Search,
    Addons,
    Favorites,
    Playlists,
    Queue,
    Settings,
    Detail(Box<DetailKey>),
}

impl Route {
    pub fn title(&self) -> &'static str {
        match self {
            Route::Home => "Inicio",
            Route::Search => "Buscar",
            Route::Addons => "Addons",
            Route::Favorites => "Favoritos",
            Route::Playlists => "Playlists",
            Route::Queue => "Cola",
            Route::Settings => "Ajustes",
            Route::Detail(_) => "Detalle",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetailKey {
    pub addon_id: String,
    pub addon_name: String,
    pub kind: ItemKind,
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub artwork: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AddonSearchHits {
    pub addon_id: String,
    pub addon_name: String,
    pub res: SearchResponse,
    pub elapsed: Duration,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct SearchState {
    pub input: String,
    pub active_query: String,
    pub gen: u64,
    pub running: usize,
    pub total: usize,
    pub hits: Vec<AddonSearchHits>,
    pub tab: ItemKind,
    pub started: Option<Instant>,
    pub last_total_ms: Option<Duration>,
}

impl SearchState {
    pub fn tracks(&self) -> Vec<(String, String, Track, Duration)> {
        let mut v = Vec::new();
        for h in &self.hits {
            for t in &h.res.tracks {
                v.push((h.addon_id.clone(), h.addon_name.clone(), t.clone(), h.elapsed));
            }
        }
        v
    }
    pub fn albums(&self) -> Vec<(String, String, Album)> {
        let mut v = Vec::new();
        for h in &self.hits {
            for a in &h.res.albums {
                v.push((h.addon_id.clone(), h.addon_name.clone(), a.clone()));
            }
        }
        v
    }
    pub fn artists(&self) -> Vec<(String, String, Artist)> {
        let mut v = Vec::new();
        for h in &self.hits {
            for a in &h.res.artists {
                v.push((h.addon_id.clone(), h.addon_name.clone(), a.clone()));
            }
        }
        v
    }
    pub fn playlists(&self) -> Vec<(String, String, Playlist)> {
        let mut v = Vec::new();
        for h in &self.hits {
            for p in &h.res.playlists {
                v.push((h.addon_id.clone(), h.addon_name.clone(), p.clone()));
            }
        }
        v
    }
}

#[derive(Debug, Clone, Default)]
pub struct Shelf {
    pub addon_id: String,
    pub addon_name: String,
    pub catalog_id: String,
    pub name: String,
    pub kind: ItemKind,
    pub items: Vec<CatalogItem>,
    pub skip: u32,
    pub end: bool,
    pub error: Option<String>,
    pub loading: bool,
}

#[derive(Debug, Default)]
pub struct HomeState {
    pub shelves: Vec<Shelf>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct DetailState {
    pub key: Option<DetailKey>,
    pub album: Option<Album>,
    pub artist: Option<Artist>,
    pub playlist: Option<Playlist>,
    pub loading: bool,
    pub error: Option<String>,
    pub elapsed: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub msg: String,
    pub error: bool,
    pub until: Instant,
}

#[derive(Debug, Default)]
pub struct Diagnostics {
    pub last_gap_ms: Option<u64>,
    pub gaps: Vec<u64>,
    pub transitions: u64,
    pub errors: u64,
    pub audio: Option<AudioInfo>,
    pub backend_line: String,
    pub resolved_via: Option<String>,
    pub resolve_ms: Option<u64>,
    pub prerolled: bool,
}

impl Diagnostics {
    pub fn avg_gap(&self) -> Option<f64> {
        if self.gaps.is_empty() {
            None
        } else {
            Some(self.gaps.iter().sum::<u64>() as f64 / self.gaps.len() as f64)
        }
    }
}

// ──────────────────────────────── App ────────────────────────────────

pub struct App {
    pub svc: Arc<Services>,
    pub route: Route,
    pub history: Vec<Route>,
    pub search: SearchState,
    pub home: HomeState,
    pub detail: DetailState,
    pub addons_view: AddonsView,
    pub toasts: Vec<Toast>,
    pub diag: Diagnostics,
    pub show_queue: bool,
    pub play_gen: u64,
    pub preroll_gen: u64,
    pub preroll_started: Option<u64>,
    /// JSON crudo del último stream resuelto (para el panel de diagnóstico).
    pub last_stream_raw: Option<String>,
    pub last_probe: Option<String>,
    pub show_debug_window: bool,
    pub search_debounce: Option<Instant>,
    pub first_frame: bool,
    pub frame_times: Vec<f32>,
    pub library_tab: usize,
    pub playlist_editing: Option<String>,
    pub focus_search: bool,
    pub settings_tab: usize,
    pub library_selected: Option<String>,
    pub library_delete: Option<String>,
}

#[derive(Debug, Default)]
pub struct AddonsView {
    pub url: String,
    pub installing: bool,
    pub expanded: Option<String>,
    pub net_tab: usize,
    pub new_header_k: String,
    pub new_header_v: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self> {
        let t0 = Instant::now();
        crate::ui::theme::install(&cc.egui_ctx);
        crate::ui::theme::load_user_font(&cc.egui_ctx, &crate::config::config_dir().join("font.ttf"));

        let svc = Services::new(&cc.egui_ctx)?;
        let prefs = svc.prefs();
        svc.player
            .sync_prefs(prefs.volume, prefs.muted, prefs.speed, &prefs.replaygain, false);
        if let Some(dev) = &prefs.audio_device {
            svc.player.backend.set_audio_device(dev);
        }

        let mut app = Self {
            svc,
            route: Route::Home,
            history: Vec::new(),
            search: SearchState::default(),
            home: HomeState::default(),
            detail: DetailState::default(),
            addons_view: AddonsView::default(),
            toasts: Vec::new(),
            diag: Diagnostics::default(),
            show_queue: true,
            play_gen: 0,
            preroll_gen: 0,
            preroll_started: None,
            last_stream_raw: None,
            last_probe: None,
            show_debug_window: false,
            search_debounce: None,
            first_frame: true,
            frame_times: Vec::with_capacity(120),
            library_tab: 0,
            playlist_editing: None,
            focus_search: false,
            settings_tab: 0,
            library_selected: None,
            library_delete: None,
        };
        app.kickoff();
        tracing::info!("arranque en {:?}", t0.elapsed());
        Ok(app)
    }

    /// Todo lo que se lanza en background al abrir la app.
    fn kickoff(&mut self) {
        let svc = self.svc.clone();
        // manifests frescos
        {
            let reg = svc.reg.clone();
            let tx = svc.atx.clone();
            let svc2 = svc.clone();
            svc.spawn(move || {
                let n = NetworkKind::detect();
                let _ = reg.refresh_manifests(n);
                let _ = tx.send(AddonResult::ManifestChanged);
                let _ = svc2;
            });
        }
        // poda del caché a disco
        {
            let svc2 = svc.clone();
            svc.spawn(move || {
                let max = svc2.prefs().cache_mb.saturating_mul(1024 * 1024);
                svc2.http.cache.prune(max);
            });
        }
        self.load_home();
    }

    // ─────────────────────────── navegación ───────────────────────────

    pub fn go(&mut self, r: Route) {
        if self.route != r {
            self.history.push(self.route.clone());
            if self.history.len() > 24 {
                self.history.remove(0);
            }
        }
        self.route = r;
    }

    pub fn back(&mut self) {
        self.route = self.history.pop().unwrap_or(Route::Home);
    }

    // ─────────────────────────── home / catálogos ───────────────────────────

    pub fn load_home(&mut self) {
        self.home.loading = true;
        self.home.error = None;
        self.home.shelves.clear();

        let svc = self.svc.clone();
        let net = svc.net();
        let addons = svc.reg.enabled();
        let mut any = false;

        for addon in addons {
            if !addon.has("catalog") {
                continue;
            }
            for def in addon.catalogs() {
                any = true;
                self.home.shelves.push(Shelf {
                    addon_id: addon.id(),
                    addon_name: addon.name(),
                    catalog_id: def.id.clone(),
                    name: def.name.clone(),
                    kind: ItemKind::parse(&def.r#type),
                    items: Vec::new(),
                    skip: 0,
                    end: false,
                    error: None,
                    loading: true,
                });
                let reg = svc.reg.clone();
                let tx = svc.atx.clone();
                let a = addon.clone();
                let d = def.clone();
                svc.spawn(move || {
                    net::spawn_catalog_blocking(reg, a, d, 0, net, tx);
                });
            }
        }
        if !any {
            self.home.loading = false;
            self.home.error = Some(
                "Ningún addon habilitado declara catálogos (`\"catalog\"` en resources). Instalá uno en Addons, o usá Buscar."
                    .to_string(),
            );
        }
    }

    pub fn load_more_shelf(&mut self, idx: usize) {
        let Some(shelf) = self.home.shelves.get(idx).cloned() else {
            return;
        };
        if shelf.loading || shelf.end {
            return;
        }
        let Some(addon) = self.svc.reg.by_id(&shelf.addon_id) else {
            return;
        };
        let skip = shelf.skip + 100;
        if let Some(s) = self.home.shelves.get_mut(idx) {
            s.loading = true;
        }
        let svc = self.svc.clone();
        let tx = svc.atx.clone();
        let net = svc.net();
        let reg = svc.reg.clone();
        let def = CatalogDef {
            id: shelf.catalog_id.clone(),
            r#type: shelf.kind.as_str().to_string(),
            name: shelf.name.clone(),
        };
        svc.spawn(move || {
            net::spawn_catalog_blocking(reg, addon, def, skip, net, tx);
        });
    }

    // ─────────────────────────── búsqueda ───────────────────────────

    pub fn do_search(&mut self, q: &str) {
        let q = q.trim().to_string();
        if q.is_empty() {
            self.search.active_query.clear();
            self.search.hits.clear();
            self.search.running = 0;
            return;
        }
        self.search.gen += 1;
        self.search.active_query = q.clone();
        self.search.hits.clear();
        self.search.started = Some(Instant::now());
        self.search.last_total_ms = None;

        let prefs = self.svc.prefs();
        let addons: Vec<Addon> = {
            let all = self.svc.reg.enabled();
            let with_search: Vec<Addon> = all.into_iter().filter(|a| a.has("search")).collect();
            if prefs.search_all_addons {
                with_search
            } else {
                match &prefs.active_addon {
                    Some(id) => with_search
                        .into_iter()
                        .filter(|a| &a.id() == id)
                        .collect(),
                    None => with_search.into_iter().take(1).collect(),
                }
            }
        };

        self.search.total = addons.len();
        self.search.running = addons.len();
        if addons.is_empty() {
            self.search.running = 0;
            self.search.hits.push(AddonSearchHits {
                addon_id: String::new(),
                addon_name: "—".into(),
                res: Default::default(),
                elapsed: Duration::ZERO,
                error: Some("No hay addons habilitados con `search` en resources.".into()),
            });
            return;
        }

        let svc = self.svc.clone();
        let net = svc.net();
        for addon in addons {
            let reg = svc.reg.clone();
            let tx = svc.atx.clone();
            let qq = q.clone();
            svc.spawn(move || {
                let t = Instant::now();
                let id = addon.id();
                let name = addon.name();
                match reg.api.search(&addon, &qq, net) {
                    Ok(mut res) => {
                        let hide = reg
                            .store
                            .config
                            .read()
                            .map(|c| c.prefs.hide_explicit)
                            .unwrap_or(false);
                        crate::addon::apply_explicit_filter(&mut res, hide);
                        let _ = tx.send(AddonResult::Search {
                            addon_id: id,
                            addon_name: name,
                            res,
                            elapsed: t.elapsed(),
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(AddonResult::SearchErr {
                            addon_id: id,
                            addon_name: name,
                            error: net::friendly_err(&e),
                            elapsed: t.elapsed(),
                        });
                    }
                }
            });
        }
    }

    // ─────────────────────────── detalle ───────────────────────────

    pub fn open_detail(&mut self, key: DetailKey) {
        self.detail = DetailState {
            key: Some(key.clone()),
            loading: true,
            ..Default::default()
        };
        self.go(Route::Detail(Box::new(key.clone())));

        let Some(addon) = self.svc.reg.by_id(&key.addon_id) else {
            self.detail.loading = false;
            self.detail.error = Some("El addon ya no está instalado.".into());
            return;
        };
        let svc = self.svc.clone();
        let tx = svc.atx.clone();
        let net = svc.net();
        let reg = svc.reg.clone();
        let kind = key.kind.clone();
        let id = key.id.clone();
        svc.spawn(move || {
            net::spawn_detail_blocking(reg, addon, kind, id, net, tx);
        });
    }

    // ─────────────────────────── reproducción ───────────────────────────

    pub fn play_tracks(&mut self, addon: &Addon, tracks: Vec<Track>, index: usize, start_at: f64) {
        if tracks.is_empty() {
            return;
        }
        let idx = index.min(tracks.len() - 1);
        let items: Vec<QueueItem> = tracks.iter().map(|t| QueueItem::new(addon, t)).collect();
        self.start_queue(items, idx, start_at);
    }

    pub fn play_catalog_items(&mut self, addon: &Addon, items: &[CatalogItem], index: usize) {
        let queue: Vec<QueueItem> = items
            .iter()
            .filter(|c| c.kind() == ItemKind::Track)
            .map(|c| QueueItem::from_catalog(addon, c))
            .collect();
        if queue.is_empty() {
            self.toast("Esa fila no es de temas reproducibles", true);
            return;
        }
        let idx = index.min(queue.len() - 1);
        self.start_queue(queue, idx, 0.0);
    }

    pub fn play_library(&mut self, tracks: &[LibraryTrack], index: usize) {
        if tracks.is_empty() {
            return;
        }
        let items: Vec<QueueItem> = tracks
            .iter()
            .map(|t| {
                let name = self
                    .svc
                    .reg
                    .by_id(&t.addon_id)
                    .map(|a| a.name())
                    .unwrap_or_else(|| t.addon_id.clone());
                QueueItem::from_library(t, &name)
            })
            .collect();
        let idx = index.min(items.len() - 1);
        self.start_queue(items, idx, 0.0);
    }

    fn start_queue(&mut self, items: Vec<QueueItem>, index: usize, start_at: f64) {
        let shuffle = self.svc.prefs().shuffle;
        self.svc.player.queue.set(items, index);
        self.svc.player.queue.set_shuffle(shuffle);
        self.preroll_started = None;
        self.play_current(start_at);
    }

    pub fn play_current(&mut self, start_at: f64) {
        let Some(item) = self.svc.player.queue.current() else {
            return;
        };
        self.play_gen += 1;
        self.preroll_gen += 1;
        self.preroll_started = None;
        self.diag.resolved_via = None;
        self.diag.resolve_ms = None;
        self.diag.prerolled = false;
        self.spawn_resolve(self.play_gen, item, start_at, false);
    }

    fn spawn_resolve(&self, gen: u64, item: QueueItem, start_at: f64, is_preroll: bool) {
        let svc = self.svc.clone();
        let tx = svc.tx.clone();
        let reg = svc.reg.clone();
        let resolver = svc.resolver.clone();
        let api_reg = svc.reg.clone();
        let prefs = svc.resolve_prefs();
        let spawner = svc.clone();
        spawner.spawn(move || {
            let chain = reg.playback_chain();
            let t = Instant::now();
            let mut outcome: Option<ResolvedStream> = None;

            // 1) el addon de origen
            if let Some(addon) = chain.iter().find(|a| a.id() == item.addon_id) {
                let track = Track {
                    id: item.track_id.clone(),
                    title: item.title.clone(),
                    artist: item.artist.clone(),
                    album: item.album.clone(),
                    duration: item.duration,
                    isrc: item.isrc.clone(),
                    ..Default::default()
                };
                match resolver.resolve_direct(&api_reg.api, addon, &track, &prefs) {
                    Ok(r) => outcome = Some(r),
                    Err(e) => tracing::debug!("resolve directo falló: {e}"),
                }
            }
            // 2) cadena de fallback por identidad
            if outcome.is_none() {
                let id = item.identity();
                match resolver.resolve_identity(&api_reg.api, &chain, &id, &prefs) {
                    Ok(r) => outcome = Some(r),
                    Err(e) => {
                        if !is_preroll {
                            let _ = tx.send(AppEvent::PlayFailed {
                                gen,
                                error: net::friendly_err(&e),
                                item: item.clone(),
                            });
                        }
                        return;
                    }
                }
            }
            let Some(mut resolved) = outcome else {
                if !is_preroll {
                    let _ = tx.send(AppEvent::PlayFailed {
                        gen,
                        error: "Ningún addon instalado puede reproducir este tema".into(),
                        item: item.clone(),
                    });
                }
                return;
            };
            resolved.resolved_in = t.elapsed();

            // verificación previa: un round-trip chico que convierte
            // "no suena" en "el CDN devolvió 403, el token expiró"
            if prefs.probe_stream {
                let probe = crate::resolve::verify_stream_adaptive(
                    &resolved.url,
                    &resolved.headers,
                    resolved.adaptive,
                );
                if let crate::resolve::Probe::Fatal(msg) = &probe {
                    tracing::warn!("stream no reproducible: {msg}");
                    if !is_preroll {
                        let _ = tx.send(AppEvent::PlayFailed {
                            gen,
                            error: msg.clone(),
                            item: item.clone(),
                        });
                        return;
                    }
                }
                resolved.probe = probe;
            }

            if is_preroll {
                let _ = tx.send(AppEvent::PrerollReady { gen, resolved });
            } else {
                let _ = tx.send(AppEvent::PlayReady {
                    gen,
                    resolved,
                    start_at,
                });
            }
        });
    }

    /// Pre-resuelve el siguiente tema cuando faltan pocos segundos (gapless).
    pub fn maybe_preroll(&mut self) {
        let p = self.svc.prefs();
        if !p.gapless || !p.crossfade_prefetch {
            return;
        }
        let st = self.svc.player.state();
        if !st.playing || st.slot.is_none() || self.svc.player.has_preroll() {
            return;
        }
        if st.duration <= 0.0 {
            return;
        }
        let left = st.duration - st.position;
        if left > p.preroll_secs.max(2.0) as f64 {
            return;
        }
        if self.preroll_started == Some(self.preroll_gen) {
            return;
        }
        self.preroll_started = Some(self.preroll_gen);
        if let Some(next) = self.svc.player.queue.next_item() {
            self.spawn_resolve(self.preroll_gen, next, 0.0, true);
        }
    }

    pub fn next_track(&mut self) {
        if !self.svc.player.queue.advance() {
            self.svc.player.stop();
            return;
        }
        self.play_current(0.0);
    }

    pub fn prev_track(&mut self) {
        if self.svc.player.position() > 3.0 {
            self.svc.player.seek(0.0);
            return;
        }
        if !self.svc.player.queue.rewind() {
            self.svc.player.seek(0.0);
            return;
        }
        self.play_current(0.0);
    }

    /// Pide por ISRC las portadas que el addon no mandó.
    pub fn enrich_artwork(&self, addon_id: &str, tracks: &[crate::models::Track]) {
        if !self.svc.prefs().artwork_lookup {
            return;
        }
        let need: Vec<(String, Option<String>)> = tracks
            .iter()
            .filter(|t| t.artwork().is_none() && t.isrc.is_some())
            .map(|t| (t.id.clone(), t.isrc.clone()))
            .collect();
        if need.is_empty() {
            return;
        }
        let svc = self.svc.clone();
        let tx = svc.atx.clone();
        let cache = svc.http.cache.clone();
        let aid = addon_id.to_string();
        svc.spawn(move || {
            let map = crate::artwork::enrich_tracks(&cache, &aid, &need);
            if !map.is_empty() {
                tracing::debug!("{} portadas resueltas por ISRC", map.len());
                let _ = tx.send(AddonResult::Artwork { map });
            }
        });
    }

    /// Portada de relleno para un tema (si el addon no trajo ninguna).
    pub fn enriched_artwork(&self, addon_id: &str, track_id: &str) -> Option<String> {
        self.svc
            .artwork
            .read()
            .ok()?
            .get(&format!("{addon_id}::{track_id}"))
            .cloned()
    }

    // ─────────────────────────── addons ───────────────────────────

    pub fn install_addon(&mut self, url: &str) {
        let url = url.trim().to_string();
        if url.is_empty() {
            return;
        }
        self.addons_view.installing = true;
        let svc = self.svc.clone();
        let tx = svc.atx.clone();
        let reg = svc.reg.clone();
        svc.spawn(move || match reg.install(&url, false) {
            Ok(a) => {
                let _ = tx.send(AddonResult::Installed {
                    addon_id: a.id(),
                    name: a.name(),
                });
            }
            Err(e) => {
                let _ = tx.send(AddonResult::InstallErr {
                    url: url.clone(),
                    error: net::friendly_err(&anyhow::anyhow!("{e}")),
                });
            }
        });
    }

    // ─────────────────────────── biblioteca ───────────────────────────

    pub fn toggle_favorite(&mut self, t: LibraryTrack) {
        let title = t.title.clone();
        let added = {
            let mut l = crate::addon::wg(&self.svc.store.library);
            l.toggle_favorite(t)
        };
        self.svc.store.mark_library_dirty();
        self.toast(
            if added {
                format!("♥ {title} en favoritos")
            } else {
                format!("{title} fuera de favoritos")
            },
            false,
        );
    }

    pub fn toggle_favorite_current(&mut self) {
        if let Some(q) = self.svc.player.queue.current() {
            let t = q.library();
            self.toggle_favorite(t);
        }
    }

    pub fn is_favorite(&self, addon_id: &str, track_id: &str) -> bool {
        crate::addon::rg(&self.svc.store.library).is_favorite(addon_id, track_id)
    }

    pub fn create_playlist(&mut self, name: String) {
        let id = format!("pl{}", crate::config::now_secs());
        self.svc.mutate_library(|l| {
            l.playlists.push(LocalPlaylist {
                id,
                name,
                tracks: Vec::new(),
                created_at: crate::config::now_secs(),
                updated_at: crate::config::now_secs(),
            });
        });
    }

    pub fn add_to_playlist(&mut self, playlist_id: &str, t: LibraryTrack) {
        let pid = playlist_id.to_string();
        self.svc.mutate_library(move |l| {
            if let Some(p) = l.playlists.iter_mut().find(|p| p.id == pid) {
                if !p.tracks.iter().any(|x| x.key() == t.key()) {
                    p.tracks.push(t);
                    p.updated_at = crate::config::now_secs();
                }
            }
        });
    }

    // ─────────────────────────── misc ───────────────────────────

    pub fn toast(&mut self, msg: impl Into<String>, error: bool) {
        let msg = msg.into();
        if error {
            tracing::warn!("{msg}");
        }
        self.toasts.push(Toast {
            msg,
            error,
            until: Instant::now() + Duration::from_secs(if error { 6 } else { 3 }),
        });
        if self.toasts.len() > 4 {
            let n = self.toasts.len() - 4;
            self.toasts.drain(0..n);
        }
    }

    pub fn current_addon(&self) -> Option<Addon> {
        let p = self.svc.prefs();
        if let Some(id) = &p.active_addon {
            if let Some(a) = self.svc.reg.by_id(id) {
                return Some(a);
            }
        }
        self.svc.reg.enabled().into_iter().next()
    }

    pub fn addons_sorted(&self) -> Vec<Addon> {
        let mut v = self.svc.reg.all();
        v.sort_by_key(|a| (a.priority(), a.name()));
        v
    }

    pub fn handle_row_action(&mut self, action: RowAction, item: QueueItem) {
        match action {
            RowAction::Favorite => {
                let fav = item.library();
                self.toggle_favorite(fav);
            }
            RowAction::Queue => {
                let title = item.title.clone();
                if self.svc.player.queue.is_empty() {
                    self.svc.player.queue.set(vec![item], 0);
                    self.play_current(0.0);
                } else {
                    self.svc.player.queue.insert_next(item);
                    self.toast(format!("«{title}» a continuación"), false);
                }
            }
            RowAction::Play | RowAction::Open | RowAction::Nothing => {}
        }
    }

    pub fn settings_map(&self, base: &str, net: NetworkKind) -> BTreeMap<String, String> {
        self.svc
            .reg
            .by_base(base)
            .map(|a| crate::addon::rg(&a.entry).settings_for(net).clone())
            .unwrap_or_default()
    }

    pub fn disk_cache_size(&self) -> u64 {
        self.svc.http.cache.size_bytes()
    }

    pub fn cache_stats(&self) -> (u64, u64, u64, u64) {
        let (t, _hosts) = crate::http::stats().snapshot();
        (t.requests, t.errors, t.cache_hits, t.bytes)
    }

    /// Aplica los eventos pendientes. Devuelve `true` si hubo alguno.
    pub fn poll_events(&mut self, ctx: &Context) -> bool {
        let (evs, addons) = self.svc.drain();
        let got = !evs.is_empty() || !addons.is_empty();
        for r in addons {
            self.on_addon_result(r);
        }
        for e in evs {
            self.handle_event(e, ctx);
        }
        got
    }

    fn on_addon_result(&mut self, r: AddonResult) {
        match r {
            AddonResult::Search {
                addon_id,
                addon_name,
                res,
                elapsed,
            } => {
                self.search.running = self.search.running.saturating_sub(1);
                let aid = addon_id.clone();
                self.search.hits.push(AddonSearchHits {
                    addon_id,
                    addon_name,
                    res: res.clone(),
                    elapsed,
                    error: None,
                });
                if self.search.running == 0 {
                    self.search.last_total_ms = self.search.started.map(|s| s.elapsed());
                }
                self.enrich_artwork(&aid, &res.tracks);
            }
            AddonResult::SearchErr {
                addon_id,
                addon_name,
                error,
                elapsed,
            } => {
                self.search.running = self.search.running.saturating_sub(1);
                self.search.hits.push(AddonSearchHits {
                    addon_id,
                    addon_name,
                    res: Default::default(),
                    elapsed,
                    error: Some(error),
                });
                if self.search.running == 0 {
                    self.search.last_total_ms = self.search.started.map(|s| s.elapsed());
                }
            }
            AddonResult::Catalog {
                addon_id,
                catalog_id,
                kind,
                items,
                end_of_row,
                skip,
                ..
            } => {
                self.home.loading = false;
                if kind == ItemKind::Track {
                    let tracks: Vec<crate::models::Track> =
                        items.iter().map(|c| c.to_track()).collect();
                    self.enrich_artwork(&addon_id, &tracks);
                }
                if let Some(s) = self
                    .home
                    .shelves
                    .iter_mut()
                    .find(|s| s.addon_id == addon_id && s.catalog_id == catalog_id)
                {
                    s.items.extend(items);
                    s.loading = false;
                    s.error = None;
                    s.end = end_of_row;
                    s.skip = skip + 100;
                }
            }
            AddonResult::CatalogErr {
                addon_id,
                catalog_id,
                error,
            } => {
                self.home.loading = false;
                if let Some(s) = self
                    .home
                    .shelves
                    .iter_mut()
                    .find(|s| s.addon_id == addon_id && s.catalog_id == catalog_id)
                {
                    s.loading = false;
                    s.error = Some(error);
                }
            }
            AddonResult::Album {
                addon_id,
                album,
                elapsed,
            } => {
                self.detail.loading = false;
                self.detail.elapsed = Some(elapsed);
                self.enrich_artwork(&addon_id, &album.tracks);
                self.detail.album = Some(album);
                self.detail.error = None;
            }
            AddonResult::Artist {
                addon_id,
                artist,
                elapsed,
            } => {
                self.detail.loading = false;
                self.detail.elapsed = Some(elapsed);
                self.enrich_artwork(&addon_id, &artist.top_tracks);
                self.detail.artist = Some(artist);
                self.detail.error = None;
            }
            AddonResult::Playlist {
                addon_id,
                playlist,
                elapsed,
            } => {
                self.detail.loading = false;
                self.detail.elapsed = Some(elapsed);
                self.enrich_artwork(&addon_id, &playlist.tracks);
                self.detail.playlist = Some(playlist);
                self.detail.error = None;
            }
            AddonResult::DetailErr { kind, id, error, .. } => {
                self.detail.loading = false;
                self.detail.error = Some(format!("{kind}/{id}: {error}"));
            }
            AddonResult::Installed { name, .. } => {
                self.addons_view.installing = false;
                self.addons_view.url.clear();
                self.toast(format!("Addon instalado: {name}"), false);
                self.load_home();
            }
            AddonResult::InstallErr { url, error } => {
                self.addons_view.installing = false;
                self.toast(format!("No se pudo instalar {url}: {error}"), true);
            }
            AddonResult::Artwork { map } => {
                if let Ok(mut m) = self.svc.artwork.write() {
                    m.extend(map);
                }
            }
            AddonResult::ManifestChanged => {
                self.addons_view.installing = false;
            }
            AddonResult::Stream { .. } | AddonResult::StreamErr { .. } => {}
            AddonResult::Toast { msg, error } => self.toast(msg, error),
        }
    }

    fn handle_event(&mut self, ev: AppEvent, ctx: &Context) {
        match ev {
            AppEvent::Player(pe) => self.on_player_event(pe),
            AppEvent::Toast { msg, error } => self.toast(msg, error),
            AppEvent::PlayReady {
                gen,
                resolved,
                start_at,
            } => {
                if gen != self.play_gen {
                    return;
                }
                let Some(item) = self.svc.player.queue.current() else {
                    return;
                };
                self.diag.resolved_via = Some(format!(
                    "{} · vía {}",
                    resolved.addon_name,
                    resolved.via.label()
                ));
                self.diag.resolve_ms = Some(resolved.resolved_in.as_millis() as u64);
                self.last_stream_raw = Some(resolved.stream.raw_json());
                self.last_probe = Some(format!(
                    "{} · {}",
                    resolved.adaptive.label(),
                    resolved.probe.label()
                ));
                if resolved.adaptive != crate::resolve::Adaptive::None {
                    tracing::info!(
                        "stream adaptativo: {} · {}",
                        resolved.adaptive.label(),
                        crate::util::ellipsize(&resolved.url, 90)
                    );
                }
                if let Some(v) = resolved.stream.video.as_ref().and_then(|v| v.url.clone()) {
                    self.diag.backend_line = format!("video disponible: {v}");
                }
                self.svc.player.play_resolved(&item, &resolved, start_at);
                self.svc.mutate_library(|l| l.record_play(&item.library()));
                ctx.request_repaint();
            }
            AppEvent::PlayFailed { gen, error, item } => {
                if gen != self.play_gen {
                    return;
                }
                self.diag.errors += 1;
                self.toast(format!("«{}»: {error}", item.title), true);
                if self.svc.player.queue.len() > 1 {
                    self.next_track();
                }
            }
            AppEvent::PrerollReady { gen, resolved } => {
                if gen != self.preroll_gen {
                    return;
                }
                let key = resolved.track_id.clone();
                self.svc.player.preroll(key, resolved);
                self.diag.prerolled = true;
            }
        }
    }

    fn on_player_event(&mut self, ev: PlayerEvent) {
        match ev {
            PlayerEvent::Playing { slot } => self.svc.player.set_slot(slot),
            PlayerEvent::Paused(p) => self.svc.player.set_paused(p),
            PlayerEvent::Position(p) => self.svc.player.set_position(p),
            PlayerEvent::Duration(d) => self.svc.player.set_duration(d),
            PlayerEvent::AudioInfo(i) => {
                self.svc.player.set_audio_info(i.clone());
                self.diag.audio = Some(i);
            }
            PlayerEvent::Buffering(b) => self.svc.player.set_buffering(b),
            PlayerEvent::Gap(ms) => {
                self.diag.last_gap_ms = Some(ms);
                self.diag.gaps.push(ms);
                if self.diag.gaps.len() > 40 {
                    self.diag.gaps.remove(0);
                }
                self.diag.transitions += 1;
            }
            PlayerEvent::Ended { slot } => self.on_track_ended(slot),
            PlayerEvent::Failed { slot, error } => {
                self.diag.errors += 1;
                self.toast(format!("Error en slot {}: {error}", slot.label()), true);
                if self.svc.player.queue.len() > 1 {
                    self.next_track();
                }
            }
            PlayerEvent::Idle => self.svc.player.set_playing(false),
        }
    }

    /// Fin de tema: promove el pre-cargado (gapless real) o resuelve el siguiente.
    fn on_track_ended(&mut self, slot: Slot) {
        let _ = slot;
        if matches!(self.svc.player.queue.repeat(), Repeat::One) {
            self.svc.player.seek(0.0);
            return;
        }
        let had_preroll = self.svc.player.take_prerolled().is_some();
        if !self.svc.player.queue.advance() {
            self.svc.player.stop();
            return;
        }
        if had_preroll {
            let new_slot = self.svc.player.state().slot.map(|s| s.other()).unwrap_or(Slot::A);
            self.svc.player.promote_prerolled();
            self.svc.player.set_slot(new_slot);
            self.diag.prerolled = false;
            if let Some(q) = self.svc.player.queue.current() {
                self.svc.mutate_library(|l| l.record_play(&q.library()));
            }
        } else {
            self.play_current(0.0);
        }
        self.preroll_started = None;
        self.preroll_gen += 1;
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // guardar configuración al cerrar
        self.svc.store.save();
    }
}

/// Utilidad para vistas: obtiene el addon por id o el activo.
pub fn addon_for(app: &App, id: &str) -> Option<Addon> {
    app.svc.reg.by_id(id).or_else(|| app.current_addon())
}
