//! Biblioteca local: favoritos, playlists y cola/recientes.

use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui, Vec2};

use crate::models::LibraryTrack;
use crate::player::QueueItem;
use crate::ui::app::App;
use crate::ui::theme::*;
use crate::ui::widgets::{self, RowAction, ROW_H_SM};

// ─────────────────────────────── favoritos ───────────────────────────────

pub fn favorites(app: &mut App, ui: &mut Ui) {
    let lib = app.svc.library();
    let favs = lib.favorites.clone();

    ui.horizontal(|ui| {
        widgets::page_title(ui, "Favoritos", Some(&format!("{} temas", favs.len())));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !favs.is_empty() && widgets::pill(ui, "▶ Reproducir todo", false).clicked() {
                app.play_library(&favs, 0);
            }
        });
    });
    ui.add_space(8.0);

    if favs.is_empty() {
        empty(ui, "♥", "Todavía no marcaste favoritos", "Tocá el corazón en cualquier tema (o presioná F con algo sonando).");
        return;
    }

    let mut remove: Option<String> = None;
    let mut play_index: Option<usize> = None;
    crate::ui::browser::track_list(ui, favs.len(), ROW_H_SM, |ui, i| {
        let t = &favs[i];
        let q = QueueItem::from_library(t, &t.addon_id);
        let fav = true;
        let (resp, action) = widgets::track_row(
            ui,
            Some(i),
            &t.title,
            &q.subtitle(),
            t.artwork_url.as_deref(),
            t.duration,
            &widgets::queue_badges(&q),
            false,
            fav,
            ROW_H_SM,
        );
        match action {
            RowAction::Play if resp.double_clicked() => play_index = Some(i),
            RowAction::Favorite => remove = Some(t.key()),
            _ => {}
        }
    });

    if let Some(i) = play_index {
        app.play_library(&favs, i);
    }
    if let Some(key) = remove {
        app.svc.mutate_library(|l| {
            l.favorites.retain(|t| t.key() != key);
        });
        app.svc.store.mark_library_dirty();
        app.toast("Quitado de favoritos", false);
    }
}

// ─────────────────────────────── playlists ───────────────────────────────

pub fn playlists(app: &mut App, ui: &mut Ui) {
    let lib = app.svc.library();
    let pls = lib.playlists.clone();

    ui.horizontal(|ui| {
        widgets::page_title(ui, "Playlists", Some(&format!("{} listas", pls.len())));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill(ui, "+ Nueva", false).clicked() {
                app.playlist_editing = Some(String::new());
            }
        });
    });

    // diálogo de creación
    if app.playlist_editing.is_some() {
        ui.add_space(6.0);
        Frame::NONE
            .fill(CARD)
            .stroke(Stroke::new(1.0, ACCENT_SOFT))
            .corner_radius(CornerRadius::same(RADIUS))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                let editing = app.playlist_editing.clone().unwrap_or_default();
                let mut name = editing;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Nombre:").size(12.5).color(TEXT_DIM));
                    let r = ui.add_sized(
                        Vec2::new(280.0, 28.0),
                        egui::TextEdit::singleline(&mut name).hint_text("Mi playlist"),
                    );
                    let ok = !name.trim().is_empty();
                    if ui
                        .add_enabled(
                            ok,
                            egui::Button::new(RichText::new("Crear").color(Color32::WHITE))
                                .fill(if ok { ACCENT } else { CARD_HOVER }),
                        )
                        .clicked()
                        || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && ok)
                    {
                        app.create_playlist(name.trim().to_string());
                        app.playlist_editing = None;
                    }
                    if widgets::pill(ui, "Cancelar", false).clicked() {
                        app.playlist_editing = None;
                    }
                });
                app.playlist_editing = Some(name);
            });
    }
    ui.add_space(8.0);

    if pls.is_empty() {
        empty(
            ui,
            "≡",
            "No tenés playlists locales",
            "Creá una y agregá temas con el botón ♡ / ⋯ de cualquier fila.",
        );
        return;
    }

    // selector de playlist
    let selected = app
        .library_selected
        .clone()
        .or_else(|| pls.first().map(|p| p.id.clone()));
    ui.horizontal(|ui| {
        for p in &pls {
            let on = selected.as_deref() == Some(p.id.as_str());
            if widgets::pill(ui, &format!("{} ({})", p.name, p.tracks.len()), on).clicked() {
                app.library_selected = Some(p.id.clone());
            }
        }
    });
    ui.add_space(8.0);

    let Some(pid) = selected.clone() else { return };
    let Some(pl) = pls.iter().find(|p| p.id == pid).cloned() else {
        return;
    };

    ui.horizontal(|ui| {
        widgets::section_header(ui, &pl.name, Some(&format!("{} temas", pl.tracks.len())));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !pl.tracks.is_empty() && widgets::pill(ui, "▶ Reproducir", false).clicked() {
                app.play_library(&pl.tracks, 0);
            }
            if widgets::pill(ui, "Poner en cola", false).clicked() {
                let items: Vec<QueueItem> = pl
                    .tracks
                    .iter()
                    .map(|t| QueueItem::from_library(t, &t.addon_id))
                    .collect();
                app.svc.player.queue.append(items);
                app.toast(format!("{} temas a la cola", pl.tracks.len()), false);
            }
            if widgets::icon_button(ui, "🗑", "Borrar playlist", false).clicked() {
                app.library_delete = Some(pid.clone());
            }
        });
    });
    ui.add_space(6.0);

    if pl.tracks.is_empty() {
        empty(
            ui,
            "♪",
            "Playlist vacía",
            "Agregá temas desde la búsqueda o los catálogos con ⋯ → «A la cola».",
        );
    } else {
        let mut remove: Option<String> = None;
        let mut play_index: Option<usize> = None;
        crate::ui::browser::track_list(ui, pl.tracks.len(), ROW_H_SM, |ui, i| {
            let t = &pl.tracks[i];
            let q = QueueItem::from_library(t, &t.addon_id);
            let (resp, action) = widgets::track_row(
                ui,
                Some(i),
                &t.title,
                &q.subtitle(),
                t.artwork_url.as_deref(),
                t.duration,
                &widgets::queue_badges(&q),
                false,
                false,
                ROW_H_SM,
            );
            match action {
                RowAction::Play if resp.double_clicked() => play_index = Some(i),
                RowAction::Favorite => remove = Some(t.key()),
                _ => {}
            }
        });
        if let Some(i) = play_index {
            app.play_library(&pl.tracks, i);
        }
        if let Some(key) = remove {
            let pid2 = pid.clone();
            app.svc.mutate_library(move |l| {
                if let Some(p) = l.playlists.iter_mut().find(|p| p.id == pid2) {
                    p.tracks.retain(|t| t.key() != key);
                    p.updated_at = crate::config::now_secs();
                }
            });
        }
    }

    if let Some(del) = app.library_delete.take() {
        app.svc.mutate_library(move |l| {
            l.playlists.retain(|p| p.id != del);
        });
        app.library_selected = None;
        app.toast("Playlist borrada", false);
    }
}

// ─────────────────────────────── cola / recientes ───────────────────────────────

pub fn queue_view(app: &mut App, ui: &mut Ui) {
    widgets::page_title(ui, "Cola y recientes", None);
    ui.add_space(6.0);

    let q = app.svc.player.queue.clone();
    ui.horizontal(|ui| {
        widgets::section_header(
            ui,
            "Cola de reproducción",
            Some(&format!(
                "{} temas · {}",
                q.len(),
                crate::util::fmt_duration(q.total_duration())
            )),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !q.is_empty() && widgets::pill(ui, "Vaciar", false).clicked() {
                q.clear();
                app.svc.player.stop();
            }
            if widgets::pill(ui, "Panel lateral", app.show_queue).clicked() {
                app.show_queue = !app.show_queue;
            }
        });
    });

    let ordered = q.ordered();
    let cursor = q.cursor();
    let n = ordered.len();
    if n == 0 {
        empty(ui, "☰", "La cola está vacía", "Reproducí algo desde Inicio, Buscar o Favoritos.");
    } else {
        let mut jump: Option<usize> = None;
        let mut remove_at: Option<usize> = None;
        crate::ui::browser::track_list(ui, n, ROW_H_SM, |ui, order_pos| {
            let (_, item) = &ordered[order_pos];
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
                app.is_favorite(&item.addon_id, &item.track_id),
                ROW_H_SM,
            );
            match action {
                RowAction::Play if resp.clicked() => jump = Some(order_pos),
                RowAction::Favorite => {
                    let lib = item.library();
                    app.toggle_favorite(lib);
                }
                RowAction::Queue if resp.secondary_clicked() => remove_at = Some(order_pos),
                _ => {}
            }
        });
        if let Some(pos) = jump {
            if q.jump(pos) {
                app.play_current(0.0);
            }
        }
        if let Some(pos) = remove_at {
            q.remove(pos);
        }
    }

    ui.add_space(16.0);
    widgets::section_header(ui, "Reproducciones recientes", None);
    let recent = app.svc.library().recent;
    if recent.is_empty() {
        empty(ui, "↻", "Sin historial todavía", "Apenas reproduzcas algo aparece acá.");
        return;
    }
    let n = recent.len().min(80);
    let mut play_index: Option<usize> = None;
    crate::ui::browser::track_list(ui, n, ROW_H_SM, |ui, i| {
        let t: &LibraryTrack = &recent[i];
        let q = QueueItem::from_library(t, &t.addon_id);
        let (resp, action) = widgets::track_row(
            ui,
            Some(i),
            &t.title,
            &q.subtitle(),
            t.artwork_url.as_deref(),
            t.duration,
            &widgets::queue_badges(&q),
            false,
            app.is_favorite(&t.addon_id, &t.track_id),
            ROW_H_SM,
        );
        if matches!(action, RowAction::Play) && resp.double_clicked() {
            play_index = Some(i);
        }
        if matches!(action, RowAction::Favorite) {
            let lib = t.clone();
            app.toggle_favorite(lib);
        }
    });
    if let Some(i) = play_index {
        let all = recent.clone();
        app.play_library(&all, i);
    }
}

fn empty(ui: &mut Ui, glyph: &str, title: &str, hint: &str) {
    ui.add_space(28.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(glyph).size(38.0).color(TEXT_FAINT));
        ui.add_space(4.0);
        ui.label(RichText::new(title).size(14.0).color(TEXT_DIM));
        ui.label(RichText::new(hint).size(11.5).color(TEXT_FAINT));
    });
}
