//! Vistas de contenido: inicio (catálogos), búsqueda, detalle y cola.

use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Stroke, Ui, Vec2};

use crate::models::*;
use crate::player::QueueItem;
use crate::ui::app::{App, DetailKey};
use crate::ui::theme::*;
use crate::ui::widgets::{self, RowAction, ROW_H, ROW_H_SM};

/// Claves de favoritos (para no capturar `App` dentro de las closures de pintado).
pub fn fav_keys(app: &App) -> std::collections::HashSet<String> {
    crate::addon::rg(&app.svc.store.library)
        .favorites
        .iter()
        .map(|t| t.key())
        .collect()
}

/// Clave del tema que está sonando.
pub fn current_key(app: &App) -> Option<String> {
    app.svc.player.queue.current().map(|q| q.key())
}

/// Portadas resueltas por ISRC (relleno cuando el addon no las manda).
pub fn artwork_map(app: &App) -> std::collections::BTreeMap<String, String> {
    app.svc
        .artwork
        .read()
        .map(|m| m.clone())
        .unwrap_or_default()
}

// ─────────────────────────────── acción diferida ───────────────────────────────

/// Las vistas no mutan `App` mientras pintan: acumulan una acción y se aplica al final.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Pending {
    #[default]
    None,
    PlayList {
        addon_id: String,
        index: usize,
    },
    OpenDetail(DetailKey),
    Favorite(QueueItem),
    Enqueue(QueueItem),
    LoadMore(usize),
}

// ─────────────────────────────── inicio ───────────────────────────────

pub fn home(app: &mut App, ui: &mut Ui) {
    let mut pending = Pending::None;

    ui.horizontal(|ui| {
        widgets::page_title(ui, "Inicio", None);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill(ui, "Actualizar", false).clicked() {
                app.load_home();
            }
            let n = app.svc.reg.enabled().len();
            ui.label(RichText::new(format!("{n} addon(s) activos")).size(11.0).color(TEXT_FAINT));
        });
    });
    ui.add_space(6.0);

    let home_err = app.home.error.clone();
    if let Some(err) = &home_err {
        let mut go_addons = false;
        Frame::NONE
            .fill(Color32::from_rgb(30, 24, 20))
            .stroke(Stroke::new(1.0, WARN))
            .corner_radius(CornerRadius::same(RADIUS))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.label(RichText::new(err).size(12.5).color(TEXT));
                if ui.button("Ir a Addons").clicked() {
                    go_addons = true;
                }
            });
        if go_addons {
            app.go(crate::ui::app::Route::Addons);
        }
        ui.add_space(8.0);
    }

    if app.home.shelves.is_empty() && app.home.loading {
        ui.horizontal(|ui| {
            widgets::spinner(ui, 16.0);
            ui.label(RichText::new("Cargando catálogos…").color(TEXT_DIM));
        });
    }

    let shelf_count = app.home.shelves.len();
    let cur_key = current_key(app);
    let favset = fav_keys(app);
    let art_lookup = artwork_map(app);
    ScrollArea::vertical()
        .auto_shrink(false)
        .id_salt("home.scroll")
        .show(ui, |ui| {
            for idx in 0..shelf_count {
                let shelf = app.home.shelves[idx].clone();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&shelf.name).text_style(ts::subtitle()).strong());
                    ui.label(
                        RichText::new(format!("· {}", shelf.addon_name))
                            .size(11.0)
                            .color(TEXT_FAINT),
                    );
                    if shelf.loading {
                        widgets::spinner(ui, 12.0);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !shelf.end && !shelf.loading && shelf.items.len() >= 100 {
                            if widgets::pill(ui, "Ver más", false).clicked() {
                                pending = Pending::LoadMore(idx);
                            }
                        }
                        ui.label(
                            RichText::new(format!("{} ítems", shelf.items.len()))
                                .size(11.0)
                                .color(TEXT_FAINT),
                        );
                    });
                });
                if let Some(e) = &shelf.error {
                    ui.label(RichText::new(e).size(11.5).color(DANGER));
                }
                ui.add_space(4.0);

                let addon_id = shelf.addon_id.clone();
                if shelf.kind == ItemKind::Track {
                    let items = shelf.items.clone();
                    let n = items.len();
                    track_list(ui, n, ROW_H, |ui, i| {
                        let it = &items[i];
                        let mut q = QueueItem {
                            addon_id: addon_id.clone(),
                            addon_name: shelf.addon_name.clone(),
                            track_id: it.id.clone(),
                            title: it.title.clone(),
                            artist: it.artist.clone(),
                            album: it.album.clone(),
                            artwork: it.artwork_url.clone(),
                            isrc: it.isrc.clone(),
                            duration: it.duration_secs(),
                            explicit: it.explicit == Some(true),
                            hi_res: it.hi_res == Some(true),
                        };
                        if q.artwork.is_none() {
                            q.artwork = art_lookup
                                .get(&format!("{addon_id}::{}", it.id))
                                .cloned();
                        }
                        row_for_queue_item(ui, i, &q, cur_key.as_deref() == Some(q.key().as_str()), favset.contains(&q.key()), &mut pending)
                    });
                } else {
                    let items = shelf.items.clone();
                    let kind = shelf.kind.clone();
                    media_grid(ui, items.len(), |ui, i| {
                        let it = &items[i];
                        let subtitle = match kind {
                            ItemKind::Album => {
                                let mut s = it.artist.clone();
                                if let Some(y) = &it.year {
                                    s.push_str(" · ");
                                    s.push_str(y);
                                }
                                s
                            }
                            ItemKind::Playlist => it.creator.clone().unwrap_or_default(),
                            _ => it.album.clone().unwrap_or_default(),
                        };
                        let resp = widgets::media_card(
                            ui,
                            widgets::GRID_W,
                            &it.title,
                            &subtitle,
                            it.artwork(),
                            None,
                        );
                        if resp.clicked() {
                            pending = Pending::OpenDetail(DetailKey {
                                addon_id: addon_id.clone(),
                                addon_name: shelf.addon_name.clone(),
                                kind: kind.clone(),
                                id: it.id.clone(),
                                title: it.title.clone(),
                                subtitle: subtitle.clone(),
                                artwork: it.artwork_url.clone(),
                            });
                        }
                    });
                }
                ui.add_space(4.0);
                ui.separator();
            }
        });

    apply(app, pending);
}

// ─────────────────────────────── búsqueda ───────────────────────────────

pub fn search(app: &mut App, ui: &mut Ui) {
    let mut pending = Pending::None;

    ui.horizontal(|ui| {
        widgets::page_title(ui, "Buscar", None);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let all = app.svc.prefs().search_all_addons;
            if widgets::pill(ui, if all { "Todos los addons" } else { "Un addon" }, all).clicked()
            {
                app.svc.set_prefs(|p| p.search_all_addons = !p.search_all_addons);
                let q = app.search.active_query.clone();
                app.do_search(&q);
            }
            let q = app.search.active_query.clone();
            if !q.is_empty() {
                ui.label(RichText::new(format!("«{q}»")).size(12.0).color(TEXT_DIM));
            }
        });
    });

    // tabs por tipo
    let counts = {
        let s = &app.search;
        [
            (ItemKind::Track, s.tracks().len(), "Temas"),
            (ItemKind::Album, s.albums().len(), "Álbumes"),
            (ItemKind::Artist, s.artists().len(), "Artistas"),
            (ItemKind::Playlist, s.playlists().len(), "Playlists"),
        ]
    };
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for (kind, n, label) in counts {
            let active = app.search.tab == kind;
            let text = if n > 0 {
                format!("{label} {n}")
            } else {
                label.to_string()
            };
            if widgets::pill(ui, &text, active).clicked() {
                app.search.tab = kind;
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if app.search.running > 0 {
                let done = app.search.total.saturating_sub(app.search.running);
                let frac = if app.search.total > 0 {
                    done as f32 / app.search.total as f32
                } else {
                    0.0
                };
                ui.set_width(120.0);
                widgets::progress_bar(ui, frac, ACCENT);
            } else if let Some(ms) = app.search.last_total_ms {
                ui.label(
                    RichText::new(format!("en {}", crate::util::fmt_latency(ms)))
                        .size(11.0)
                        .monospace()
                        .color(TEXT_FAINT),
                );
            }
        });
    });
    ui.add_space(8.0);

    // errores por addon
    for h in app.search.hits.iter() {
        if let Some(e) = &h.error {
            ui.horizontal(|ui| {
                widgets::badge(ui, &h.addon_name, DANGER);
                ui.label(RichText::new(e).size(11.5).color(DANGER));
                ui.label(
                    RichText::new(crate::util::fmt_latency(h.elapsed))
                        .size(10.5)
                        .monospace()
                        .color(TEXT_FAINT),
                );
            });
        }
    }

    if app.search.active_query.is_empty() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("⌕").size(46.0).color(TEXT_FAINT));
            ui.label(RichText::new("Escribí arriba o presioná /").size(13.0).color(TEXT_DIM));
            ui.label(
                RichText::new("Se busca en paralelo en todos los addons habilitados; cada respuesta aparece apenas llega.")
                    .size(11.5)
                    .color(TEXT_FAINT),
            );
        });
        return;
    }

    let cur_key = current_key(app);
    let favset = fav_keys(app);
    let tab = app.search.tab.clone();
    let tracks = app.search.tracks();
    let albums = app.search.albums();
    let artists = app.search.artists();
    let playlists = app.search.playlists();
    let still_running = app.search.running;
    let art_lookup = artwork_map(app);
    ScrollArea::vertical()
        .auto_shrink(false)
        .id_salt("search.scroll")
        .show(ui, |ui| match tab {
            ItemKind::Track => {
                let rows = &tracks;
                let n = rows.len();
                if n == 0 && still_running == 0 {
                    empty_state(ui, "Sin temas para esa búsqueda");
                }
                track_list(ui, n, ROW_H, |ui, i| {
                    let (addon_id, addon_name, t, _elapsed) = &rows[i];
                    let mut q = QueueItem {
                        addon_id: addon_id.clone(),
                        addon_name: addon_name.clone(),
                        track_id: t.id.clone(),
                        title: t.title.clone(),
                        artist: t.artist.clone(),
                        album: t.album.clone(),
                        artwork: t.artwork_url.clone(),
                        isrc: t.isrc.clone(),
                        duration: t.duration_secs(),
                        explicit: t.explicit == Some(true),
                        hi_res: t.hi_res == Some(true),
                    };
                    if q.artwork.is_none() {
                        q.artwork = art_lookup
                            .get(&format!("{addon_id}::{}", t.id))
                            .cloned();
                    }
                    row_for_queue_item(ui, i, &q, cur_key.as_deref() == Some(q.key().as_str()), favset.contains(&q.key()), &mut pending)
                });
            }
            ItemKind::Album => {
                let rows = &albums;
                let n = rows.len();
                if n == 0 && still_running == 0 {
                    empty_state(ui, "Sin álbumes");
                }
                media_grid(ui, n, |ui, i| {
                    let (addon_id, addon_name, a) = &rows[i];
                    let resp = widgets::media_card(
                        ui,
                        widgets::GRID_W,
                        &a.title,
                        &widgets::album_subtitle(a),
                        a.artwork(),
                        None,
                    );
                    if resp.clicked() {
                        pending = Pending::OpenDetail(DetailKey {
                            addon_id: addon_id.clone(),
                            addon_name: addon_name.clone(),
                            kind: ItemKind::Album,
                            id: a.id.clone(),
                            title: a.title.clone(),
                            subtitle: widgets::album_subtitle(a),
                            artwork: a.artwork_url.clone(),
                        });
                    }
                });
            }
            ItemKind::Artist => {
                let rows = &artists;
                let n = rows.len();
                if n == 0 && still_running == 0 {
                    empty_state(ui, "Sin artistas");
                }
                media_grid(ui, n, |ui, i| {
                    let (addon_id, addon_name, a) = &rows[i];
                    let resp = widgets::media_card(
                        ui,
                        widgets::GRID_W,
                        &a.name,
                        &widgets::artist_subtitle(a),
                        a.artwork(),
                        None,
                    );
                    if resp.clicked() {
                        pending = Pending::OpenDetail(DetailKey {
                            addon_id: addon_id.clone(),
                            addon_name: addon_name.clone(),
                            kind: ItemKind::Artist,
                            id: a.id.clone(),
                            title: a.name.clone(),
                            subtitle: widgets::artist_subtitle(a),
                            artwork: a.artwork_url.clone(),
                        });
                    }
                });
            }
            _ => {
                let rows = &playlists;
                let n = rows.len();
                if n == 0 && still_running == 0 {
                    empty_state(ui, "Sin playlists");
                }
                media_grid(ui, n, |ui, i| {
                    let (addon_id, addon_name, p) = &rows[i];
                    let resp = widgets::media_card(
                        ui,
                        widgets::GRID_W,
                        &p.title,
                        &widgets::playlist_subtitle(p),
                        p.artwork(),
                        None,
                    );
                    if resp.clicked() {
                        pending = Pending::OpenDetail(DetailKey {
                            addon_id: addon_id.clone(),
                            addon_name: addon_name.clone(),
                            kind: ItemKind::Playlist,
                            id: p.id.clone(),
                            title: p.title.clone(),
                            subtitle: widgets::playlist_subtitle(p),
                            artwork: p.artwork_url.clone(),
                        });
                    }
                });
            }
        });

    apply(app, pending);
}

fn empty_state(ui: &mut Ui, msg: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(msg).size(13.0).color(TEXT_DIM));
    });
}

// ─────────────────────────────── detalle ───────────────────────────────

pub fn detail(app: &mut App, ui: &mut Ui, key: DetailKey) {
    let mut pending = Pending::None;

    // cabecera
    ui.horizontal(|ui| {
        widgets::cover(ui, key.artwork.as_deref(), 132.0, RADIUS + 2, &key.title);
        ui.add_space(14.0);
        ui.vertical(|ui| {
            ui.label(
                RichText::new(key.kind.as_str().to_ascii_uppercase())
                    .size(10.5)
                    .color(ACCENT)
                    .strong(),
            );
            ui.label(RichText::new(&key.title).text_style(ts::title()).strong());
            ui.label(RichText::new(&key.subtitle).size(13.0).color(TEXT_DIM));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                widgets::badge(ui, &key.addon_name, TEXT_FAINT);
                if let Some(ms) = app.detail.elapsed {
                    widgets::badge(ui, &crate::util::fmt_latency(ms), ACCENT_SOFT)
                        .on_hover_text("latencia del endpoint de detalle");
                }
                if app.detail.loading {
                    widgets::spinner(ui, 12.0);
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can_play = match key.kind {
                    ItemKind::Album => app.detail.album.as_ref().map(|a| !a.tracks.is_empty()),
                    ItemKind::Playlist => app
                        .detail
                        .playlist
                        .as_ref()
                        .map(|p| !p.tracks.is_empty()),
                    ItemKind::Artist => app
                        .detail
                        .artist
                        .as_ref()
                        .map(|a| !a.top_tracks.is_empty()),
                    _ => None,
                };
                if can_play.unwrap_or(false) {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("  ▶  Reproducir  ").color(Color32::WHITE))
                                .fill(ACCENT)
                                .corner_radius(CornerRadius::same(20))
                                .min_size(Vec2::new(140.0, 32.0)),
                        )
                        .clicked()
                    {
                        pending = Pending::PlayList {
                            addon_id: key.addon_id.clone(),
                            index: 0,
                        };
                    }
                }
                if widgets::pill(ui, "Recargar", false).clicked() {
                    let k = key.clone();
                    app.open_detail(k);
                }
            });
        });
    });

    let detail_err = app.detail.error.clone();
    if let Some(e) = &detail_err {
        ui.add_space(8.0);
        Frame::NONE
            .fill(Color32::from_rgb(34, 20, 24))
            .stroke(Stroke::new(1.0, DANGER))
            .corner_radius(CornerRadius::same(RADIUS))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.label(RichText::new(e).size(12.0).color(TEXT));
                ui.label(
                    RichText::new("El addon no implementa este endpoint, o el id no existe.")
                        .size(11.0)
                        .color(TEXT_FAINT),
                );
            });
    }

    ui.add_space(10.0);

    let cur_key = current_key(app);
    let favset = fav_keys(app);
    let art_lookup = artwork_map(app);
    match key.kind {
        ItemKind::Album => {
            let album = app.detail.album.clone();
            let loading = app.detail.loading;
            let tracks = album.as_ref().map(|a| a.tracks.clone()).unwrap_or_default();
            if loading && tracks.is_empty() {
                loading_rows(ui);
            }
            let n = tracks.len();
            let aid = key.addon_id.clone();
            let aname = key.addon_name.clone();
            let album_title = album.as_ref().map(|a| a.title.clone());
            let art = album.as_ref().and_then(|a| a.artwork_url.clone());
            {
                track_list(ui, n, ROW_H_SM, |ui, i| {
                    let t = &tracks[i];
                    let mut q = QueueItem {
                        addon_id: aid.clone(),
                        addon_name: aname.clone(),
                        track_id: t.id.clone(),
                        title: t.title.clone(),
                        artist: t.artist.clone(),
                        album: album_title.clone(),
                        artwork: t.artwork_url.clone().or_else(|| art.clone()),
                        isrc: t.isrc.clone(),
                        duration: t.duration_secs(),
                        explicit: t.explicit == Some(true),
                        hi_res: t.hi_res == Some(true),
                    };
                    if q.artwork.is_none() {
                        q.artwork = art_lookup.get(&format!("{aid}::{}", t.id)).cloned();
                    }
                    row_for_queue_item(ui, i, &q, cur_key.as_deref() == Some(q.key().as_str()), favset.contains(&q.key()), &mut pending)
                });
            }
        }
        ItemKind::Playlist => {
            let plist = app.detail.playlist.clone();
            let loading = app.detail.loading;
            let tracks = plist.as_ref().map(|p| p.tracks.clone()).unwrap_or_default();
            if loading && tracks.is_empty() {
                loading_rows(ui);
            }
            let n = tracks.len();
            let aid = key.addon_id.clone();
            let aname = key.addon_name.clone();
            let pl_title = plist.as_ref().map(|p| p.title.clone());
            let art = plist.as_ref().and_then(|p| p.artwork_url.clone());
            {
                track_list(ui, n, ROW_H_SM, |ui, i| {
                    let t = &tracks[i];
                    let mut q = QueueItem {
                        addon_id: aid.clone(),
                        addon_name: aname.clone(),
                        track_id: t.id.clone(),
                        title: t.title.clone(),
                        artist: t.artist.clone(),
                        album: pl_title.clone(),
                        artwork: t.artwork_url.clone().or_else(|| art.clone()),
                        isrc: t.isrc.clone(),
                        duration: t.duration_secs(),
                        explicit: t.explicit == Some(true),
                        hi_res: t.hi_res == Some(true),
                    };
                    if q.artwork.is_none() {
                        q.artwork = art_lookup.get(&format!("{aid}::{}", t.id)).cloned();
                    }
                    row_for_queue_item(ui, i, &q, cur_key.as_deref() == Some(q.key().as_str()), favset.contains(&q.key()), &mut pending)
                });
            }
        }
        ItemKind::Artist => {
            let artist = app.detail.artist.clone();
            let loading = app.detail.loading;
            let top = artist.as_ref().map(|a| a.top_tracks.clone()).unwrap_or_default();
            let albums = artist.as_ref().map(|a| a.albums.clone()).unwrap_or_default();
            let bio = artist.as_ref().and_then(|a| a.bio.clone());
            let genres = artist.as_ref().map(|a| a.genres.clone()).unwrap_or_default();
            if loading && top.is_empty() {
                loading_rows(ui);
            }
            if !genres.is_empty() {
                ui.horizontal(|ui| {
                    for g in genres.iter().take(6) {
                        widgets::badge(ui, g, ACCENT_2);
                    }
                });
            }
            if let Some(b) = bio.filter(|b| !b.trim().is_empty()) {
                ui.add_space(4.0);
                ui.label(RichText::new(crate::util::ellipsize(&b, 600)).size(12.0).color(TEXT_DIM));
            }
            ui.add_space(8.0);
            widgets::section_header(ui, "Temas populares", Some(&format!("{}", top.len())));
            let n = top.len();
            let aid = key.addon_id.clone();
            let aname = key.addon_name.clone();
            {
                track_list(ui, n, ROW_H_SM, |ui, i| {
                    let t = &top[i];
                    let mut q = QueueItem {
                        addon_id: aid.clone(),
                        addon_name: aname.clone(),
                        track_id: t.id.clone(),
                        title: t.title.clone(),
                        artist: t.artist.clone(),
                        album: t.album.clone(),
                        artwork: t.artwork_url.clone(),
                        isrc: t.isrc.clone(),
                        duration: t.duration_secs(),
                        explicit: t.explicit == Some(true),
                        hi_res: t.hi_res == Some(true),
                    };
                    if q.artwork.is_none() {
                        q.artwork = art_lookup.get(&format!("{aid}::{}", t.id)).cloned();
                    }
                    row_for_queue_item(ui, i, &q, cur_key.as_deref() == Some(q.key().as_str()), favset.contains(&q.key()), &mut pending)
                });

                if !albums.is_empty() {
                    ui.add_space(12.0);
                    widgets::section_header(ui, "Álbumes", Some(&format!("{}", albums.len())));
                    let aid2 = key.addon_id.clone();
                    let aname2 = key.addon_name.clone();
                    media_grid(ui, albums.len(), |ui, i| {
                        let a = &albums[i];
                        let resp = widgets::media_card(
                            ui,
                            widgets::GRID_W,
                            &a.title,
                            &widgets::album_subtitle(a),
                            a.artwork(),
                            None,
                        );
                        if resp.clicked() {
                            pending = Pending::OpenDetail(DetailKey {
                                addon_id: aid2.clone(),
                                addon_name: aname2.clone(),
                                kind: ItemKind::Album,
                                id: a.id.clone(),
                                title: a.title.clone(),
                                subtitle: widgets::album_subtitle(a),
                                artwork: a.artwork_url.clone(),
                            });
                        }
                    });
                }
            }
        }
        _ => {}
    }

    apply(app, pending);
}

fn loading_rows(ui: &mut Ui) {
    ui.horizontal(|ui| {
        widgets::spinner(ui, 14.0);
        ui.label(RichText::new("Cargando del addon…").size(12.0).color(TEXT_DIM));
    });
}

// ─────────────────────────────── cola ───────────────────────────────

pub fn queue_panel(app: &mut App, ui: &mut Ui) {
    let mut pending = Pending::None;
    let q = app.svc.player.queue.clone();

    ui.horizontal(|ui| {
        ui.label(RichText::new("Cola").text_style(ts::subtitle()).strong());
        ui.label(RichText::new(format!("{}", q.len())).size(11.0).color(TEXT_FAINT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::icon_button(ui, "✕", "Ocultar panel (Q)", false).clicked() {
                app.show_queue = false;
            }
            if widgets::icon_button(ui, "🗑", "Vaciar cola", false).clicked() && !q.is_empty() {
                q.clear();
                app.svc.player.stop();
            }
        });
    });

    let total = q.total_duration();
    if total > 0.0 {
        ui.label(
            RichText::new(crate::util::fmt_duration(total))
                .size(10.5)
                .monospace()
                .color(TEXT_FAINT),
        );
    }
    ui.add_space(6.0);

    let ordered = q.ordered();
    let cursor = q.cursor();
    let n = ordered.len();
    let favset = fav_keys(app);
    ScrollArea::vertical()
        .auto_shrink(false)
        .id_salt("queue.scroll")
        .show_rows(ui, ROW_H_SM, n, |ui, range| {
            for (order_pos, (_, item)) in ordered.iter().enumerate().skip(range.start).take(range.end - range.start) {
                let is_cur = order_pos == cursor;
                let (resp, action) = widgets::track_row(
                    ui,
                    Some(order_pos),
                    &item.title,
                    &item.subtitle(),
                    item.artwork.as_deref(),
                    item.duration,
                    &widgets::queue_badges(item),
                    is_cur,
                    favset.contains(&item.key()),
                    ROW_H_SM,
                );
                match action {
                    RowAction::Play => {
                        if resp.clicked() && q.jump(order_pos) {
                            app.play_current(0.0);
                        }
                    }
                    RowAction::Favorite => pending = Pending::Favorite(item.clone()),
                    RowAction::Queue => {}
                    _ => {}
                }
            }
        });

    if n == 0 {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("☰").size(28.0).color(TEXT_FAINT));
            ui.label(RichText::new("La cola está vacía").size(12.0).color(TEXT_DIM));
        });
    }
    apply(app, pending);
}

pub fn queue_view(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        widgets::page_title(ui, "Cola y recientes", None);
    });
    ui.add_space(6.0);
    queue_panel(app, ui);
    ui.add_space(12.0);
    let cur_key = current_key(app);
    let favset = fav_keys(app);
    widgets::section_header(ui, "Reproducciones recientes", None);
    let recent = app.svc.library().recent;
    let n = recent.len().min(60);
    let mut pending = Pending::None;
    track_list(ui, n, ROW_H_SM, |ui, i| {
        let t = &recent[i];
        let q = QueueItem::from_library(t, &t.addon_id);
        row_for_queue_item(
            ui,
            i,
            &q,
            cur_key.as_deref() == Some(q.key().as_str()),
            favset.contains(&q.key()),
            &mut pending,
        );
    });
    apply(app, pending);
}

// ─────────────────────────────── helpers de lista ───────────────────────────────

/// Lista virtualizada de filas de altura fija.
pub fn track_list(
    ui: &mut Ui,
    total: usize,
    row_h: f32,
    mut row: impl FnMut(&mut Ui, usize),
) {
    ScrollArea::vertical()
        .auto_shrink(false)
        .id_salt(ui.id().with("track_list"))
        .show_rows(ui, row_h, total, |ui, range| {
            for i in range {
                row(ui, i);
            }
        });
}

/// Grilla de tarjetas que ajusta columnas al ancho (virtualizada por filas).
pub fn media_grid(ui: &mut Ui, total: usize, mut cell: impl FnMut(&mut Ui, usize)) {
    if total == 0 {
        return;
    }
    let gap = 14.0;
    let card_w = widgets::GRID_W;
    let cols = widgets::grid_widths(ui.available_width(), card_w, gap).max(1);
    let rows = total.div_ceil(cols);
    let row_h = card_w + 52.0 + gap;
    ScrollArea::vertical()
        .auto_shrink(false)
        .id_salt(ui.id().with("media_grid"))
        .show_rows(ui, row_h, rows, |ui, range| {
            for r in range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for c in 0..cols {
                        let idx = r * cols + c;
                        if idx >= total {
                            // reservamos el espacio para que no se desalinee
                            ui.allocate_space(Vec2::new(card_w, row_h - gap));
                            continue;
                        }
                        cell(ui, idx);
                    }
                });
            }
        });
}

/// Fila para un `QueueItem` (búsqueda, catálogos, detalles).
#[allow(clippy::too_many_arguments)]
fn row_for_queue_item(
    ui: &mut Ui,
    index: usize,
    q: &QueueItem,
    is_current: bool,
    fav: bool,
    pending: &mut Pending,
) {
    let badges = widgets::queue_badges(q);
    let (resp, action) = widgets::track_row(
        ui,
        Some(index),
        &q.title,
        &q.subtitle(),
        q.artwork.as_deref(),
        q.duration,
        &badges,
        is_current,
        fav,
        ROW_H,
    );
    match action {
        RowAction::Play => {
            let addon_id = q.addon_id.clone();
            if resp.double_clicked() {
                *pending = Pending::PlayList { addon_id, index };
            } else {
                *pending = Pending::Enqueue(q.clone());
            }
        }
        RowAction::Favorite => *pending = Pending::Favorite(q.clone()),
        RowAction::Queue => *pending = Pending::Enqueue(q.clone()),
        _ => {}
    }
}



/// Aplica la acción acumulada por la vista.
fn apply(app: &mut App, pending: Pending) {
    match pending {
        Pending::None => {}
        Pending::LoadMore(idx) => app.load_more_shelf(idx),
        Pending::OpenDetail(key) => app.open_detail(key),
        Pending::Favorite(item) => app.handle_row_action(RowAction::Favorite, item),
        Pending::Enqueue(item) => {
            // un click agrega a la cola; si la cola está vacía, reproduce
            if app.svc.player.queue.is_empty() {
                let addon_id = item.addon_id.clone();
                play_items(app, &addon_id, vec![item], 0);
            } else {
                app.handle_row_action(RowAction::Queue, item);
            }
        }
        Pending::PlayList { addon_id, index } => play_from_context(app, &addon_id, index),
    }
}

/// Reproduce la lista completa visible en la vista actual, empezando en `index`.
fn play_from_context(app: &mut App, addon_id: &str, index: usize) {
    let Some(addon) = app.svc.reg.by_id(addon_id) else {
        app.toast("Ese addon ya no está instalado", true);
        return;
    };
    match app.route.clone() {
        crate::ui::app::Route::Search => {
            let rows = app.search.tracks();
            let tracks: Vec<Track> = rows
                .iter()
                .filter(|(id, _, _, _)| id == addon_id)
                .map(|(_, _, t, _)| t.clone())
                .collect();
            let idx = index.min(tracks.len().saturating_sub(1));
            app.play_tracks(&addon, tracks, idx, 0.0);
        }
        crate::ui::app::Route::Home => {
            // busca en todas las shelves de tracks y reproduce la que contiene el índice
            let mut acc = 0usize;
            let shelves = app.home.shelves.clone();
            for s in shelves {
                if s.kind != ItemKind::Track {
                    continue;
                }
                if index < acc + s.items.len() {
                    let local = index - acc;
                    app.play_catalog_items(&addon, &s.items, local);
                    return;
                }
                acc += s.items.len();
            }
        }
        crate::ui::app::Route::Detail(key) => {
            match key.kind {
                ItemKind::Album => {
                    if let Some(a) = &app.detail.album {
                        app.play_tracks(&addon, a.tracks.clone(), index, 0.0);
                    }
                }
                ItemKind::Playlist => {
                    if let Some(p) = &app.detail.playlist {
                        app.play_tracks(&addon, p.tracks.clone(), index, 0.0);
                    }
                }
                ItemKind::Artist => {
                    if let Some(a) = &app.detail.artist {
                        app.play_tracks(&addon, a.top_tracks.clone(), index, 0.0);
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
    let _ = &addon;
}

fn play_items(app: &mut App, addon_id: &str, items: Vec<QueueItem>, index: usize) {
    let Some(addon) = app.svc.reg.by_id(addon_id) else {
        app.svc.player.queue.set(items, index);
        app.play_current(0.0);
        return;
    };
    let tracks: Vec<Track> = items
        .iter()
        .map(|q| Track {
            id: q.track_id.clone(),
            title: q.title.clone(),
            artist: q.artist.clone(),
            album: q.album.clone(),
            duration: q.duration,
            artwork_url: q.artwork.clone(),
            isrc: q.isrc.clone(),
            ..Default::default()
        })
        .collect();
    app.play_tracks(&addon, tracks, index, 0.0);
}

