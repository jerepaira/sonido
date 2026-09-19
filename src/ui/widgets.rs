//! Widgets reutilizables de la UI.

use egui::{
    Align, Align2, Color32, CornerRadius, Layout, Pos2, Rect, Response, Sense, Stroke, StrokeKind,
    Ui, UiBuilder, Vec2,
};

use crate::models::{Album, Artist, CatalogItem, Playlist, Track};
use crate::player::QueueItem;
use crate::ui::theme::*;

pub const ROW_H: f32 = 58.0;
pub const ROW_H_SM: f32 = 46.0;
pub const GRID_W: f32 = 164.0;

pub use crate::ui::browser::{media_grid, track_list};

/// Rectángulo redondeado con relleno y borde.
pub fn card_bg(ui: &Ui, rect: Rect, fill: Color32, stroke: Option<Stroke>, radius: u8) {
    ui.painter().rect_filled(rect, CornerRadius::same(radius), fill);
    if let Some(s) = stroke {
        ui.painter()
            .rect_stroke(rect, CornerRadius::same(radius), s, StrokeKind::Inside);
    }
}

/// Fila/tarjeta clickeable con fondo.
pub fn card(
    ui: &mut Ui,
    min_height: f32,
    active: bool,
    content: impl FnOnce(&mut Ui),
) -> Response {
    let desired = Vec2::new(ui.available_width(), min_height);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());
    let hovered = resp.hovered();
    let fill = if active {
        ACCENT.linear_multiply(0.20)
    } else if hovered {
        CARD_HOVER
    } else {
        CARD
    };
    let stroke = if active {
        Some(Stroke::new(1.0, ACCENT))
    } else if hovered {
        Some(Stroke::new(1.0, BORDER))
    } else {
        None
    };
    card_bg(ui, rect, fill, stroke, RADIUS);

    ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink(8.0))
            .layout(Layout::left_to_right(Align::Center)),
    )
    .scope(content);
    resp
}

/// Portada con placeholder mientras carga (o si falla).
pub fn cover(ui: &mut Ui, url: Option<&str>, size: f32, radius: u8, fallback: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    let rr = CornerRadius::same(radius);

    // placeholder: dos tonos superpuestos simulan un degradé
    painter.rect_filled(rect, rr, Color32::from_rgb(30, 33, 46));
    painter.rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(rect.width(), rect.height() * 0.55)),
        rr,
        Color32::from_rgb(38, 42, 58),
    );

    match url.filter(|u| !u.is_empty()) {
        Some(u) => {
            egui::Image::from_uri(u.to_string())
                .fit_to_exact_size(Vec2::splat(size))
                .corner_radius(rr)
                .bg_fill(Color32::from_rgb(24, 26, 36))
                .show_loading_spinner(false)
                .alt_text(fallback)
                .paint_at(ui, rect);

            // Si la carga falló, lo marcamos en rojo y el tooltip dice por qué.
            // Sin esto, una portada que no carga parece "la app no muestra imágenes".
            use egui::load::{SizeHint, TexturePoll};
            #[allow(unused_imports)]
            let load = ui
                .ctx()
                .try_load_texture(u, egui::TextureOptions::LINEAR, SizeHint::Scale(1.0.into()));
            match load {
                Err(e) => {
                    let corner = Rect::from_min_size(
                        rect.right_top() - Vec2::splat(11.0),
                        Vec2::splat(11.0),
                    );
                    ui.painter().circle_filled(corner.center(), 4.5, DANGER);
                    resp.clone().on_hover_text(format!(
                        "No se pudo cargar la portada\n{e}\n\nURL: {u}"
                    ));
                }
                Ok(TexturePoll::Pending { .. }) => ui.ctx().request_repaint(),
                Ok(TexturePoll::Ready { .. }) => {}
            }
        }
        None => {
            let initials: String = fallback
                .split_whitespace()
                .take(2)
                .filter_map(|w| w.chars().next())
                .collect::<String>()
                .to_uppercase();
            let text = if initials.trim().is_empty() {
                "♪".to_string()
            } else {
                initials
            };
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                text,
                egui::FontId::new(size * 0.32, egui::FontFamily::Proportional),
                TEXT_FAINT,
            );
        }
    }
    resp
}

/// Etiqueta pequeña con fondo de color.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) -> Response {
    if text.is_empty() {
        return ui.allocate_exact_size(Vec2::ZERO, Sense::hover()).1;
    }
    let font = egui::FontId::new(10.0, egui::FontFamily::Proportional);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let pad = Vec2::new(5.0, 2.0);
    let size = galley.size() + pad * 2.0;
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(4),
        color.linear_multiply(0.16),
    );
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0, color.linear_multiply(0.45)),
        StrokeKind::Inside,
    );
    ui.painter()
        .galley_with_override_text_color(rect.min + pad, galley, color);
    resp
}

/// Botón de glifo con tooltip.
pub fn icon_button(ui: &mut Ui, glyph: &str, tip: &str, active: bool) -> Response {
    let mut b = egui::Button::new(
        egui::RichText::new(glyph)
            .size(14.0)
            .color(if active { ACCENT } else { TEXT_DIM }),
    )
    .corner_radius(CornerRadius::same(8));
    if active {
        b = b.fill(ACCENT.linear_multiply(0.18));
    }
    ui.add(b).on_hover_text(tip)
}

pub fn icon_button_sized(
    ui: &mut Ui,
    glyph: &str,
    tip: &str,
    active: bool,
    size: f32,
) -> Response {
    let mut b = egui::Button::new(
        egui::RichText::new(glyph)
            .size(size)
            .color(if active { Color32::WHITE } else { TEXT }),
    )
    .corner_radius(CornerRadius::same((size * 0.42) as u8))
    .min_size(Vec2::splat(size * 1.7));
    if active {
        b = b.fill(ACCENT);
    }
    ui.add(b).on_hover_text(tip)
}

/// Botón "pill" (para tabs y filtros).
pub fn pill(ui: &mut Ui, text: &str, active: bool) -> Response {
    let b = egui::Button::new(
        egui::RichText::new(text)
            .size(12.5)
            .color(if active { Color32::WHITE } else { TEXT_DIM }),
    )
    .fill(if active { ACCENT } else { Color32::TRANSPARENT })
    .stroke(Stroke::new(1.0, if active { ACCENT } else { BORDER }))
    .corner_radius(CornerRadius::same(20));
    ui.add(b)
}

/// Barra de progreso finita.
pub fn progress_bar(ui: &mut Ui, frac: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 3.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), BORDER);
    let w = rect.width() * frac.clamp(0.0, 1.0);
    if w > 0.5 {
        painter.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(w, rect.height())),
            CornerRadius::same(2),
            color,
        );
    }
    resp
}

/// Spinner propio (barato: 10 círculos).
pub fn spinner(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let c = rect.center();
    for i in 0..10 {
        let a = t * 3.4 + i as f32 * 0.628;
        let alpha = ((10 - i) as f32 / 10.0 * 210.0) as u8;
        let p = c + Vec2::new(a.cos(), a.sin()) * (size * 0.36);
        ui.painter().circle_filled(
            p,
            size * 0.075,
            Color32::from_rgba_unmultiplied(124, 92, 255, alpha),
        );
    }
    ui.ctx().request_repaint();
}

pub fn section_header(ui: &mut Ui, title: &str, sub: Option<&str>) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).text_style(ts::subtitle()).strong());
        if let Some(s) = sub {
            ui.label(egui::RichText::new(s).size(11.5).color(TEXT_FAINT));
        }
    });
}

/// Título grande de vista.
pub fn page_title(ui: &mut Ui, title: &str, sub: Option<&str>) {
    ui.label(egui::RichText::new(title).text_style(ts::title()).strong());
    if let Some(s) = sub {
        ui.label(egui::RichText::new(s).size(12.0).color(TEXT_DIM));
    }
}

// ─────────────────────────────── filas ───────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    Nothing,
    Play,
    Open,
    Queue,
    Favorite,
}

/// Fila de tema genérica. `badges` = pares (texto, color).
#[allow(clippy::too_many_arguments)]
pub fn track_row(
    ui: &mut Ui,
    index: Option<usize>,
    title: &str,
    subtitle: &str,
    artwork: Option<&str>,
    duration: Option<f64>,
    badges: &[(String, Color32)],
    is_current: bool,
    is_favorite: bool,
    height: f32,
) -> (Response, RowAction) {
    let mut action = RowAction::Nothing;
    let resp = card(ui, height, is_current, |ui| {
        // índice
        ui.allocate_ui_with_layout(
            Vec2::new(26.0, height - 16.0),
            Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                let label = match index {
                    Some(i) => format!("{:>2}", i + 1),
                    None => "♪".to_string(),
                };
                ui.label(egui::RichText::new(label).size(11.5).color(TEXT_FAINT));
            },
        );

        cover(ui, artwork, height - 16.0, RADIUS_SM, title);
        ui.add_space(8.0);

        // título + subtítulo
        let text_w = (ui.available_width() - 240.0).max(120.0);
        ui.allocate_ui_with_layout(
            Vec2::new(text_w, height - 16.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(text_w);
                ui.label(
                    egui::RichText::new(crate::util::ellipsize(title, 70))
                        .size(13.5)
                        .strong()
                        .color(if is_current { ACCENT } else { TEXT }),
                );
                ui.label(
                    egui::RichText::new(crate::util::ellipsize(subtitle, 90))
                        .size(11.5)
                        .color(TEXT_DIM),
                );
            },
        );

        // derecha: duración, badges y acciones
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, "⋯", "Agregar a la cola", false).clicked() {
                action = RowAction::Queue;
            }
            if icon_button(
                ui,
                if is_favorite { "♥" } else { "♡" },
                if is_favorite {
                    "Quitar de favoritos"
                } else {
                    "Agregar a favoritos"
                },
                is_favorite,
            )
            .clicked()
            {
                action = RowAction::Favorite;
            }
            for (b, c) in badges.iter().rev() {
                badge(ui, b, *c);
            }
            ui.label(
                egui::RichText::new(crate::util::fmt_duration_opt(duration))
                    .size(11.5)
                    .monospace()
                    .color(TEXT_FAINT),
            );
        });
    });

    if resp.double_clicked() {
        action = RowAction::Play;
    } else if resp.clicked() {
        action = RowAction::Play;
    }
    (resp, action)
}

/// Tarjeta para grillas (álbum / artista / playlist).
pub fn media_card(
    ui: &mut Ui,
    width: f32,
    title: &str,
    subtitle: &str,
    artwork: Option<&str>,
    badge_text: Option<&str>,
) -> Response {
    let total_h = width + 52.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, total_h), Sense::click());
    if resp.hovered() {
        card_bg(ui, rect, CARD_HOVER, None, RADIUS);
    }
    let cover_rect = Rect::from_min_size(
        rect.min + Vec2::new(0.0, 3.0),
        Vec2::new(width, width),
    );
    let mut cui = ui.new_child(UiBuilder::new().max_rect(cover_rect).layout(Layout::default()));
    cover(&mut cui, artwork, width, RADIUS, title);

    if let Some(b) = badge_text.filter(|b| !b.is_empty()) {
        let font = egui::FontId::new(10.0, egui::FontFamily::Proportional);
        let g = ui.painter().layout_no_wrap(b.to_string(), font, Color32::WHITE);
        let size = g.size() + Vec2::new(8.0, 4.0);
        let r = Rect::from_min_size(cover_rect.right_bottom() - size - Vec2::new(5.0, 5.0), size);
        ui.painter()
            .rect_filled(r, CornerRadius::same(5), Color32::from_black_alpha(175));
        ui.painter()
            .galley_with_override_text_color(r.min + Vec2::new(4.0, 2.0), g, Color32::WHITE);
    }

    let t = ui.painter();
    let top = cover_rect.bottom() + 7.0;
    t.text(
        Pos2::new(rect.left() + 2.0, top),
        Align2::LEFT_TOP,
        crate::util::ellipsize(title, 32),
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        TEXT,
    );
    t.text(
        Pos2::new(rect.left() + 2.0, top + 17.0),
        Align2::LEFT_TOP,
        crate::util::ellipsize(subtitle, 40),
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
        TEXT_DIM,
    );
    resp
}

/// Grilla que ajusta columnas al ancho disponible.
pub fn grid_widths(available: f32, card_w: f32, gap: f32) -> usize {
    ((available + gap) / (card_w + gap)).floor().max(1.0) as usize
}

// ─────────────────────────────── helpers de badges ───────────────────────────────

pub fn track_badges(addon_name: &str, t: &Track) -> Vec<(String, Color32)> {
    let mut v = Vec::new();
    if let Some(f) = t.format.as_deref().filter(|s| !s.is_empty()) {
        v.push((f.to_ascii_uppercase(), ACCENT_2));
    }
    if t.hi_res == Some(true) {
        v.push(("HI-RES".into(), OK));
    }
    if t.explicit == Some(true) {
        v.push(("E".into(), DANGER));
    }
    v.push((crate::util::ellipsize(addon_name, 14), TEXT_FAINT));
    v
}

pub fn catalog_badges(addon_name: &str, c: &CatalogItem) -> Vec<(String, Color32)> {
    let mut v = Vec::new();
    if c.hi_res == Some(true) {
        v.push(("HI-RES".into(), OK));
    }
    if c.explicit == Some(true) {
        v.push(("E".into(), DANGER));
    }
    if c.isrc.is_some() {
        v.push(("ISRC".into(), ACCENT_2));
    }
    v.push((crate::util::ellipsize(addon_name, 14), TEXT_FAINT));
    v
}

pub fn queue_badges(q: &QueueItem) -> Vec<(String, Color32)> {
    let mut v = Vec::new();
    if q.hi_res {
        v.push(("HI-RES".into(), OK));
    }
    if q.explicit {
        v.push(("E".into(), DANGER));
    }
    v.push((crate::util::ellipsize(&q.addon_name, 14), TEXT_FAINT));
    v
}

pub fn album_subtitle(a: &Album) -> String {
    let mut s = a.artist.clone();
    let y = a.year_str();
    if !y.is_empty() {
        s.push_str(" · ");
        s.push_str(&y);
    }
    if let Some(n) = a.track_count {
        s.push_str(&format!(" · {n} temas"));
    }
    s
}

pub fn artist_subtitle(a: &Artist) -> String {
    if a.genres.is_empty() {
        format!("{} temas", a.top_tracks.len())
    } else {
        a.genres.iter().take(3).cloned().collect::<Vec<_>>().join(" · ")
    }
}

pub fn playlist_subtitle(p: &Playlist) -> String {
    let mut s = p.creator.clone().unwrap_or_default();
    if let Some(n) = p.track_count {
        if !s.is_empty() {
            s.push_str(" · ");
        }
        s.push_str(&format!("{n} temas"));
    }
    s
}
