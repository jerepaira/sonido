//! Tema visual: oscuro, alto contraste y sin adornos que cuesten frames.

use egui::{
    Color32, CornerRadius, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals,
};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const ACCENT: Color32 = Color32::from_rgb(124, 92, 255);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(88, 68, 178);
pub const ACCENT_2: Color32 = Color32::from_rgb(0, 214, 190);
pub const BG: Color32 = Color32::from_rgb(10, 11, 16);
pub const PANEL: Color32 = Color32::from_rgb(15, 17, 24);
pub const CARD: Color32 = Color32::from_rgb(22, 25, 35);
pub const CARD_HOVER: Color32 = Color32::from_rgb(31, 35, 49);
pub const BORDER: Color32 = Color32::from_rgb(38, 42, 58);
pub const TEXT: Color32 = Color32::from_rgb(233, 236, 244);
pub const TEXT_DIM: Color32 = Color32::from_rgb(148, 156, 176);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(104, 112, 132);
pub const DANGER: Color32 = Color32::from_rgb(255, 92, 110);
pub const OK: Color32 = Color32::from_rgb(70, 220, 150);
pub const WARN: Color32 = Color32::from_rgb(255, 186, 72);

pub const RADIUS: u8 = 10;
pub const RADIUS_SM: u8 = 6;

/// Estilos de texto propios (egui 0.36 usa `TextStyle::Name(Arc<str>)`).
pub mod ts {
    use egui::TextStyle;
    use std::sync::{Arc, OnceLock};

    pub fn title() -> TextStyle {
        static S: OnceLock<TextStyle> = OnceLock::new();
        S.get_or_init(|| TextStyle::Name(Arc::from("sonido-title"))).clone()
    }
    pub fn subtitle() -> TextStyle {
        static S: OnceLock<TextStyle> = OnceLock::new();
        S.get_or_init(|| TextStyle::Name(Arc::from("sonido-subtitle")))
            .clone()
    }
    pub fn tiny() -> TextStyle {
        static S: OnceLock<TextStyle> = OnceLock::new();
        S.get_or_init(|| TextStyle::Name(Arc::from("sonido-tiny"))).clone()
    }
}

pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = visuals();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size = egui::vec2(40.0, 22.0);
    style.spacing.slider_width = 120.0;
    style.spacing.combo_width = 90.0;
    style.spacing.scroll.bar_width = 10.0;
    style.spacing.scroll.handle_min_length = 24.0;
    style.spacing.scroll.bar_inner_margin = 2.0;
    style.spacing.scroll.floating = true;
    style.spacing.window_margin = egui::Margin::same(12);
    style.text_styles = text_styles();
    style.override_text_style = None;
    style.drag_value_text_style = TextStyle::Body;
    style.animation_time = 0.12;
    ctx.set_style_of(egui::Theme::Dark, style);

    // Tipografía embebida de egui: no toca fontconfig => arranque instantáneo.
    let fonts = FontDefinitions::default();
    ctx.set_fonts(fonts);

    // el fondo de la ventana
    ctx.set_visuals_of(egui::Theme::Dark, visuals());
    let _ = BG;
}

fn text_styles() -> BTreeMap<TextStyle, FontId> {
    let mut t = BTreeMap::new();
    t.insert(
        TextStyle::Small,
        FontId::new(11.5, egui::FontFamily::Proportional),
    );
    t.insert(TextStyle::Body, FontId::new(13.5, egui::FontFamily::Proportional));
    t.insert(
        TextStyle::Button,
        FontId::new(13.5, egui::FontFamily::Proportional),
    );
    t.insert(
        TextStyle::Monospace,
        FontId::new(12.5, egui::FontFamily::Monospace),
    );
    t.insert(
        TextStyle::Heading,
        FontId::new(20.0, egui::FontFamily::Proportional),
    );
    t.insert(ts::title(), FontId::new(26.0, egui::FontFamily::Proportional));
    t.insert(ts::subtitle(), FontId::new(15.0, egui::FontFamily::Proportional));
    t.insert(ts::tiny(), FontId::new(10.5, egui::FontFamily::Proportional));
    t
}

pub fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.dark_mode = true;
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(12, 13, 19);
    v.faint_bg_color = CARD;
    v.code_bg_color = CARD;
    v.hyperlink_color = ACCENT_2;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.selection.bg_fill = ACCENT.linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    v.popup_shadow = v.window_shadow;
    v.override_text_color = Some(TEXT);

    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(RADIUS);

    v.widgets.inactive.bg_fill = CARD;
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.corner_radius = CornerRadius::same(RADIUS_SM);
    v.widgets.inactive.expansion = 0.0;

    v.widgets.hovered.bg_fill = CARD_HOVER;
    v.widgets.hovered.weak_bg_fill = CARD_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT_SOFT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.4, Color32::WHITE);
    v.widgets.hovered.corner_radius = CornerRadius::same(RADIUS_SM);
    v.widgets.hovered.expansion = 1.0;

    v.widgets.active.bg_fill = ACCENT_SOFT;
    v.widgets.active.weak_bg_fill = ACCENT_SOFT;
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.active.fg_stroke = Stroke::new(1.4, Color32::WHITE);
    v.widgets.active.corner_radius = CornerRadius::same(RADIUS_SM);

    v.widgets.open.bg_fill = CARD_HOVER;
    v.widgets.open.weak_bg_fill = CARD_HOVER;
    v.widgets.open.bg_stroke = Stroke::new(1.0, ACCENT_SOFT);
    v.widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.open.corner_radius = CornerRadius::same(RADIUS_SM);

    v
}

/// Si el usuario deja una TTF/OTF en `~/.config/sonido/font.ttf`, se usa.
pub fn load_user_font(ctx: &egui::Context, path: &std::path::Path) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "user".to_owned(),
        Arc::new(egui::FontData::from_owned(bytes)),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "user".to_owned());
    ctx.set_fonts(fonts);
    tracing::info!("fuente personalizada cargada: {}", path.display());
}
