//! Estructura general de la ventana: sidebar, barra superior, contenido,
//! panel de cola, barra de reproducción y toasts.

use std::time::Instant;

use egui::{
    Align, Area, Color32, CornerRadius, Frame, Id, Layout, Margin, Order, Panel, Pos2, Rect,
    RichText, Stroke, Ui, Vec2,
};

use crate::net::NetworkKind;
use crate::ui::app::{App, Route};
use crate::ui::theme::*;
use crate::ui::widgets;

const SIDEBAR_W: f32 = 226.0;
const TOPBAR_H: f32 = 54.0;
const PLAYERBAR_H: f32 = 94.0;

/// Punto de entrada de cada frame.
pub fn ui(app: &mut App, ui: &mut Ui) {
    let t0 = Instant::now();
    let ctx = ui.ctx().clone();

    if app.focus_search {
        app.focus_search = false;
        ctx.memory_mut(|m| m.request_focus(Id::new("sonido.search")));
    }

    handle_keys(app, &ctx);

    Panel::left("sidebar")
        .exact_size(SIDEBAR_W)
        .resizable(false)
        .frame(
            Frame::NONE
                .fill(BG)
                .inner_margin(Margin::symmetric(10, 12))
                .stroke(Stroke::new(1.0, Color32::from_rgb(24, 26, 36))),
        )
        .show(ui, |ui| sidebar(app, ui));

    if app.show_queue {
        Panel::right("queue")
            .default_size(330.0)
            .size_range(260.0..=520.0)
            .frame(
                Frame::NONE
                    .fill(Color32::from_rgb(12, 13, 19))
                    .inner_margin(Margin::symmetric(10, 10))
                    .stroke(Stroke::new(1.0, BORDER)),
            )
            .show(ui, |ui| crate::ui::browser::queue_panel(app, ui));
    }

    Panel::bottom("playerbar")
        .exact_size(PLAYERBAR_H)
        .resizable(false)
        .frame(
            Frame::NONE
                .fill(PANEL)
                .inner_margin(Margin::symmetric(14, 10))
                .stroke(Stroke::new(1.0, BORDER)),
        )
        .show(ui, |ui| crate::ui::playerbar::ui(app, ui));

    Panel::top("topbar")
        .exact_size(TOPBAR_H)
        .resizable(false)
        .frame(Frame::NONE.fill(PANEL).inner_margin(Margin::symmetric(14, 8)))
        .show(ui, |ui| topbar(app, ui));

    // contenido central
    let inner = Frame::NONE.fill(BG).inner_margin(Margin::symmetric(16, 12));
    inner.show(ui, |ui| content(app, ui));

    // búsqueda con debounce
    if let Some(at) = app.search_debounce.take() {
        if at.elapsed().as_millis() >= 220 {
            let q = app.search.input.clone();
            if q.trim() != app.search.active_query.trim() {
                app.do_search(&q);
            }
        } else {
            app.search_debounce = Some(at);
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        }
    }

    if app.show_debug_window {
        debug_window(app, &ctx);
    }

    toasts(app, &ctx);

    app.frame_times.push(t0.elapsed().as_secs_f32() * 1000.0);
    if app.frame_times.len() > 120 {
        app.frame_times.remove(0);
    }
    app.first_frame = false;

    let st = app.svc.player.state();
    if st.playing || app.search.running > 0 || app.home.loading || app.detail.loading {
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

// ─────────────────────────────── sidebar ───────────────────────────────

fn nav_button(ui: &mut Ui, glyph: &str, label: &str, active: bool) -> bool {
    let b = egui::Button::new(
        RichText::new(format!("  {glyph}   {label}"))
            .size(13.5)
            .color(if active { Color32::WHITE } else { TEXT_DIM }),
    )
    .fill(if active {
        ACCENT.linear_multiply(0.22)
    } else {
        Color32::TRANSPARENT
    })
    .stroke(Stroke::NONE)
    .corner_radius(CornerRadius::same(8))
    .min_size(Vec2::new(ui.available_width(), 34.0));
    ui.add(b).clicked()
}

fn sidebar(app: &mut App, ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            // marca "eclipse": círculo con media luna
            let (r, _) = ui.allocate_exact_size(Vec2::splat(24.0), egui::Sense::hover());
            let p = ui.painter();
            p.circle_filled(r.center(), 11.0, ACCENT);
            p.circle_filled(r.center() + Vec2::new(5.0, -3.0), 9.5, BG);
            ui.label(RichText::new("Sonido").size(19.0).strong().color(TEXT));
        });
        ui.label(
            RichText::new("música nativa · addons")
                .size(10.5)
                .color(TEXT_FAINT),
        );
    });
    ui.add_space(14.0);

    let route = app.route.clone();
    let is = |r: &Route| std::mem::discriminant(&route) == std::mem::discriminant(r);

    if nav_button(ui, "⌂", "Inicio", is(&Route::Home)) {
        app.go(Route::Home);
    }
    if nav_button(ui, "⌕", "Buscar", is(&Route::Search)) {
        app.go(Route::Search);
        app.focus_search = true;
    }

    ui.add_space(8.0);
    ui.label(
        RichText::new("  TU BIBLIOTECA")
            .size(10.0)
            .color(TEXT_FAINT)
            .strong(),
    );
    ui.add_space(2.0);
    if nav_button(ui, "♥", "Favoritos", is(&Route::Favorites)) {
        app.go(Route::Favorites);
    }
    if nav_button(ui, "≡", "Playlists", is(&Route::Playlists)) {
        app.go(Route::Playlists);
    }
    if nav_button(ui, "☰", "Cola", matches!(route, Route::Queue)) {
        app.go(Route::Queue);
    }

    ui.add_space(8.0);
    ui.label(
        RichText::new("  FUENTES")
            .size(10.0)
            .color(TEXT_FAINT)
            .strong(),
    );
    ui.add_space(2.0);
    if nav_button(ui, "⧉", "Addons", is(&Route::Addons)) {
        app.go(Route::Addons);
    }
    ui.add_space(8.0);

    // selector de addon activo
    let prefs = app.svc.prefs();
    let addons = app.addons_sorted();
    let enabled: Vec<_> = addons.iter().filter(|a| a.enabled()).cloned().collect();
    let label = if prefs.search_all_addons {
        "Todos los addons".to_string()
    } else {
        prefs
            .active_addon
            .as_ref()
            .and_then(|id| enabled.iter().find(|a| &a.id() == id))
            .map(|a| a.name())
            .or_else(|| enabled.first().map(|a| a.name()))
            .unwrap_or_else(|| "Sin addons".to_string())
    };
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        egui::ComboBox::from_id_salt("sidebar.addon")
            .selected_text(RichText::new(label).size(12.0).color(TEXT))
            .width(SIDEBAR_W - 48.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(prefs.search_all_addons, "Todos los addons")
                    .clicked()
                {
                    app.svc.set_prefs(|p| p.search_all_addons = true);
                }
                ui.separator();
                for a in &enabled {
                    let id = a.id();
                    let on = !prefs.search_all_addons
                        && prefs.active_addon.as_deref() == Some(id.as_str());
                    if ui.selectable_label(on, a.name()).clicked() {
                        app.svc.set_prefs(move |p| {
                            p.search_all_addons = false;
                            p.active_addon = Some(id);
                        });
                    }
                }
            });
    });

    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
        let net = NetworkKind::detect();
        let (reqs, errs, hits, bytes) = app.cache_stats();
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            let dot = if errs > 0 && errs * 4 > reqs.max(1) {
                WARN
            } else {
                OK
            };
            let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 3.5, dot);
            ui.label(
                RichText::new(format!(
                    "{} · {} req · {} hit",
                    net.label(),
                    reqs,
                    hits
                ))
                .size(10.0)
                .color(TEXT_FAINT),
            )
            .on_hover_text(format!(
                "peticiones HTTP: {reqs}\nfallidas: {errs}\nhits de caché: {hits}\ndescargado: {}\nmotor: {}\nuptime: {} s",
                crate::util::fmt_bytes(bytes),
                app.svc.player.state().backend,
                app.svc.started.elapsed().as_secs()
            ));
        });
        if nav_button(ui, "⚙", "Ajustes", matches!(app.route, Route::Settings)) {
            app.go(Route::Settings);
        }
        ui.add_space(4.0);
    });
}

// ─────────────────────────────── topbar ───────────────────────────────

fn topbar(app: &mut App, ui: &mut Ui) {
    let mut reload = false;
    ui.horizontal(|ui| {
        if widgets::icon_button(ui, "←", "Atrás (Esc)", false).clicked() {
            app.back();
        }
        if widgets::icon_button(ui, "⌂", "Inicio", false).clicked() {
            app.go(Route::Home);
        }
        if widgets::icon_button(ui, "⟳", "Recargar vista", false).clicked() {
            reload = true;
        }
        ui.add_space(10.0);

        let search_w = ui.available_width().min(560.0).max(180.0);
        let resp = ui
            .add_sized(
                Vec2::new(search_w, 30.0),
                egui::TextEdit::singleline(&mut app.search.input)
                    .id(Id::new("sonido.search"))
                    .hint_text(RichText::new("  ⌕  Buscar temas, álbumes, artistas…").color(TEXT_FAINT)),
            )
            .on_hover_text("Enter para buscar · / o ⌘K para venir acá");

        if resp.changed() {
            app.search_debounce = Some(Instant::now());
            if app.route != Route::Search {
                app.go(Route::Search);
            }
        }
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let q = app.search.input.clone();
            app.search_debounce = None;
            app.do_search(&q);
        }

        if app.search.running > 0 {
            widgets::spinner(ui, 14.0);
            ui.label(
                RichText::new(format!("{} en curso", app.search.running))
                    .size(11.0)
                    .color(TEXT_FAINT),
            );
        } else if let Some(ms) = app.search.last_total_ms {
            ui.label(
                RichText::new(crate::util::fmt_latency(ms))
                    .size(11.0)
                    .monospace()
                    .color(TEXT_FAINT),
            )
            .on_hover_text("tiempo total de la última búsqueda (addons en paralelo)");
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            diag_menu(app, ui);
            let st = app.svc.player.state();
            if let Some(a) = &st.audio {
                let b = a.badge();
                if !b.is_empty() {
                    widgets::badge(ui, &b, if a.bit_depth >= 24 { OK } else { ACCENT_2 })
                        .on_hover_text(format!(
                            "codec {:?} · {} Hz · {} ch",
                            a.codec, a.sample_rate, a.channels
                        ));
                }
            }
            if st.preloaded {
                widgets::badge(ui, "GAPLESS", OK).on_hover_text(
                    "el siguiente tema ya está pre-cargado en el motor: la transición no tendrá corte",
                );
            }
        });
    });

    if reload {
        match app.route.clone() {
            Route::Home => app.load_home(),
            Route::Search => {
                let q = app.search.active_query.clone();
                app.do_search(&q);
            }
            Route::Addons => {
                let reg = app.svc.reg.clone();
                let tx = app.svc.atx.clone();
                app.svc.spawn(move || {
                    let n = NetworkKind::detect();
                    let _ = reg.refresh_manifests(n);
                    let _ = tx.send(crate::net::AddonResult::ManifestChanged);
                });
                app.toast("Actualizando manifests…", false);
            }
            Route::Detail(k) => app.open_detail(*k),
            _ => {}
        }
    }
}

fn diag_menu(app: &mut App, ui: &mut Ui) {
    ui.menu_button(RichText::new("ⓘ").size(14.0).color(TEXT_DIM), |ui| {
        ui.set_min_width(340.0);
        let st = app.svc.player.state();
        ui.label(RichText::new("Diagnóstico").strong());
        ui.separator();
        let line = |ui: &mut Ui, k: &str, v: String| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(k).size(11.5).color(TEXT_DIM));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(v).size(11.5).monospace().color(TEXT));
                });
            });
        };
        line(ui, "motor", st.backend.clone());
        line(ui, "backend", app.svc.player.backend.backend_stats());
        line(
            ui,
            "gap último",
            app.diag
                .last_gap_ms
                .map(|g| format!("{g} ms"))
                .unwrap_or_else(|| "—".into()),
        );
        line(
            ui,
            "gap promedio",
            app.diag
                .avg_gap()
                .map(|g| format!("{g:.0} ms"))
                .unwrap_or_else(|| "—".into()),
        );
        line(ui, "transiciones", app.diag.transitions.to_string());
        line(ui, "errores", app.diag.errors.to_string());
        line(
            ui,
            "stream",
            app.diag.resolved_via.clone().unwrap_or_else(|| "—".into()),
        );
        line(
            ui,
            "resolución",
            app.diag
                .resolve_ms
                .map(|m| format!("{m} ms"))
                .unwrap_or_else(|| "—".into()),
        );
        let (reqs, errs, hits, bytes) = app.cache_stats();
        line(ui, "http", format!("{reqs} req · {errs} err"));
        line(
            ui,
            "caché http",
            format!("{hits} hits · {}", crate::util::fmt_bytes(bytes)),
        );
        let (dec, dh, ie, ib, n) = app.svc.image_loader.stats();
        line(
            ui,
            "imágenes",
            format!("{dec} dec · {dh} disco · {ie} err · {n} mem"),
        );
        line(ui, "imágenes bytes", crate::util::fmt_bytes(ib));
        let avg = if app.frame_times.is_empty() {
            0.0
        } else {
            app.frame_times.iter().sum::<f32>() / app.frame_times.len() as f32
        };
        line(ui, "frame ui", format!("{avg:.2} ms"));
        line(ui, "uptime", format!("{} s", app.svc.started.elapsed().as_secs()));
        ui.separator();
        if ui.button("Vaciar caché HTTP").clicked() {
            app.svc.http.cache.clear();
            app.svc.resolver.cache.clear();
            app.toast("Caché HTTP vaciado", false);
            ui.close();
        }
        if ui.button("Vaciar caché de imágenes").clicked() {
            app.svc.image_loader.forget_all_egui(ui.ctx());
            ui.close();
        }
        ui.separator();
        if ui.button("Depuración de addon / stream…").clicked() {
            app.show_debug_window = true;
            ui.close();
        }
    });
}

/// Ventana de depuración: JSON crudo del último stream, verificación de la URL,
/// portadas que fallaron y atajos de consola.
fn debug_window(app: &mut App, ctx: &egui::Context) {
    let mut open = true;
    egui::Window::new("Depuración")
        .open(&mut open)
        .default_size([720.0, 520.0])
        .show(ctx, |ui| {
            ui.label(RichText::new("Último stream resuelto").strong());
            ui.label(
                RichText::new(
                    "Si la URL termina en .mpd o .m3u8 (típico de Tidal), no es un archivo de audio:\n\
                     es un manifiesto de streaming adaptativo. La app lo detecta y le pasa a mpv\n\
                     `demuxer=dash`/`demuxer=hls`; mpv baja y descifra los segmentos mientras suena.",
                )
                .size(10.5)
                .color(TEXT_FAINT),
            );
            if let Some(p) = &app.last_probe {
                widgets::badge(ui, p, if p.starts_with("no reproducible") { DANGER } else { OK });
            }
            if let Some(v) = &app.diag.resolved_via {
                widgets::badge(ui, v, TEXT_FAINT);
            }
            ui.add_space(4.0);
            let raw = app.last_stream_raw.clone().unwrap_or_else(|| {
                "(todavía no se resolvió ningún stream — reproducí algo)".to_string()
            });
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .id_salt("dbg.raw")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(RichText::new(raw).size(11.0).monospace().color(TEXT_DIM));
                });

            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("Portadas que no se pudieron cargar").strong());
            let fails = app.svc.image_loader.failures();
            if fails.is_empty() {
                ui.label(
                    RichText::new("ninguna — todas las portadas cargaron o todavía no se pidieron")
                        .size(11.5)
                        .color(TEXT_FAINT),
                );
            } else {
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .id_salt("dbg.img")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (url, err) in fails.iter().rev() {
                            ui.horizontal(|ui| {
                                widgets::badge(ui, "ERR", DANGER);
                                ui.label(
                                    RichText::new(crate::util::ellipsize(url, 78))
                                        .size(10.5)
                                        .monospace()
                                        .color(TEXT_DIM),
                                )
                                .on_hover_text(url.clone());
                                ui.label(RichText::new(err).size(10.5).color(DANGER));
                            });
                        }
                    });
            }

            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("Estado del loader de imágenes").strong());
            let (dec, dh, ie, ib, n) = app.svc.image_loader.stats();
            ui.label(
                RichText::new(format!(
                    "decodificadas {dec} · desde disco {dh} · errores {ie} · en memoria {n} · {}",
                    crate::util::fmt_bytes(ib)
                ))
                .size(11.5)
                .monospace()
                .color(TEXT_DIM),
            );

            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("Diagnóstico profundo por consola").strong());
            let addon_id = app
                .current_addon()
                .map(|a| a.id())
                .unwrap_or_else(|| "<addon>".into());
            let last_q = app.search.active_query.clone();
            let q = if last_q.is_empty() { "<texto>".to_string() } else { last_q };
            for cmd in [
                format!("sonido doctor \"{q}\" --addon {addon_id}"),
                format!("sonido dump search --addon {addon_id} -q q=<texto>"),
                format!("sonido dump stream/<trackId> --addon {addon_id}"),
                format!("sonido stream --addon {addon_id} <trackId>"),
            ] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&cmd).size(11.0).monospace().color(ACCENT_2));
                    if widgets::icon_button(ui, "⧉", "Copiar al portapapeles", false).clicked() {
                        ui.ctx().copy_text(cmd.clone());
                    }
                });
            }
            ui.label(
                RichText::new("`doctor` recorre manifest → settings → search → stream → verificación de la URL → portada, y muestra el JSON crudo de cada paso.")
                    .size(10.5)
                    .color(TEXT_FAINT),
            );
        });
    app.show_debug_window = open;
}

// ─────────────────────────────── contenido ───────────────────────────────

fn content(app: &mut App, ui: &mut Ui) {
    match app.route.clone() {
        Route::Home => crate::ui::browser::home(app, ui),
        Route::Search => crate::ui::browser::search(app, ui),
        Route::Addons => crate::ui::addons::view(app, ui),
        Route::Favorites => crate::ui::library::favorites(app, ui),
        Route::Playlists => crate::ui::library::playlists(app, ui),
        Route::Queue => crate::ui::library::queue_view(app, ui),
        Route::Settings => crate::ui::settings::view(app, ui),
        Route::Detail(key) => crate::ui::browser::detail(app, ui, *key),
    }
}

// ─────────────────────────────── toasts ───────────────────────────────

fn toasts(app: &mut App, ctx: &egui::Context) {
    app.toasts.retain(|t| t.until > Instant::now());
    if app.toasts.is_empty() {
        return;
    }
    let screen = ctx.input(|i| i.viewport_rect());
    let mut y = screen.max.y - PLAYERBAR_H - 26.0;
    for (n, t) in app.toasts.iter().rev().take(4).enumerate() {
        let w = 430.0f32.min(screen.width() - 40.0);
        let h = 40.0;
        let pos = Pos2::new(screen.center().x - w / 2.0, y - h);
        y -= h + 8.0;
        Area::new(Id::new("toast").with(n))
            .fixed_pos(pos)
            .order(Order::Tooltip)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(if t.error {
                        Color32::from_rgb(48, 20, 28)
                    } else {
                        Color32::from_rgb(24, 28, 40)
                    })
                    .stroke(Stroke::new(
                        1.0,
                        if t.error { DANGER } else { ACCENT_SOFT },
                    ))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_min_width(w - 24.0);
                        ui.label(
                            RichText::new(crate::util::ellipsize(&t.msg, 96))
                                .size(12.5)
                                .color(TEXT),
                        );
                    });
            });
    }
    ctx.request_repaint();
}

// ─────────────────────────────── atajos ───────────────────────────────

fn handle_keys(app: &mut App, ctx: &egui::Context) {
    let typing = ctx.input(|i| i.focused) && ctx.memory(|m| m.focused().is_some());
    ctx.input(|i| {
        if i.key_pressed(egui::Key::Escape) && !typing {
            if app.route != Route::Home {
                app.back();
            }
        }
        if typing {
            return;
        }
        let cmd = i.modifiers.command;
        if i.key_pressed(egui::Key::Space) {
            app.svc.player.toggle_pause();
        }
        if cmd && i.key_pressed(egui::Key::ArrowRight) {
            app.next_track();
        }
        if cmd && i.key_pressed(egui::Key::ArrowLeft) {
            app.prev_track();
        }
        if i.modifiers.shift && i.key_pressed(egui::Key::ArrowRight) {
            app.svc.player.seek_by(10.0);
        }
        if i.modifiers.shift && i.key_pressed(egui::Key::ArrowLeft) {
            app.svc.player.seek_by(-10.0);
        }
        if i.key_pressed(egui::Key::F) {
            app.toggle_favorite_current();
        }
        if i.key_pressed(egui::Key::M) {
            let m = !app.svc.player.state().muted;
            app.svc.player.set_muted(m);
            app.svc.set_prefs(move |p| p.muted = m);
        }
        if i.key_pressed(egui::Key::S) {
            let s = !app.svc.prefs().shuffle;
            app.svc.set_prefs(|p| p.shuffle = s);
            app.svc.player.queue.set_shuffle(s);
        }
        if i.key_pressed(egui::Key::R) {
            let r = app.svc.player.queue.repeat().next();
            let rs = r.as_str().to_string();
            app.svc.player.queue.set_repeat(r);
            app.svc.set_prefs(move |p| p.repeat = rs);
        }
        if i.key_pressed(egui::Key::Q) {
            app.show_queue = !app.show_queue;
        }
        if i.key_pressed(egui::Key::Slash) || (cmd && i.key_pressed(egui::Key::K)) {
            app.go(Route::Search);
            app.focus_search = true;
        }
        if i.key_pressed(egui::Key::Num1) {
            app.go(Route::Home);
        }
        if i.key_pressed(egui::Key::Num2) {
            app.go(Route::Search);
        }
        if i.key_pressed(egui::Key::Num3) {
            app.go(Route::Addons);
        }
    });
}

/// Barra fina de progreso (posición / buffer / volumen).
pub fn thin_bar(ui: &Ui, rect: Rect, frac: f32, buffered: Option<f32>, color: Color32) {
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(3), Color32::from_rgb(45, 49, 66));
    if let Some(b) = buffered {
        let w = rect.width() * (b.clamp(0.0, 1.0) as f32);
        if w > 0.5 {
            p.rect_filled(
                Rect::from_min_size(rect.min, Vec2::new(w, rect.height())),
                CornerRadius::same(3),
                Color32::from_rgb(62, 68, 92),
            );
        }
    }
    let w = rect.width() * frac.clamp(0.0, 1.0);
    if w > 0.5 {
        p.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(w, rect.height())),
            CornerRadius::same(3),
            color,
        );
        p.circle_filled(rect.min + Vec2::new(w, rect.height() / 2.0), 5.0, color);
    }
}
