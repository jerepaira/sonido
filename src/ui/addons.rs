//! Gestión de addons: instalar por URL, habilitar, ordenar prioridad y configurar
//! los settings que declara cada manifest.

use egui::{
    Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Stroke, Ui, Vec2,
};

use crate::addon::rg;
use crate::models::{SettingDef, SettingType};
use crate::net::NetworkKind;
use crate::ui::app::App;
use crate::ui::theme::*;
use crate::ui::widgets;

/// Acciones que se aplican después de pintar (para no pelear con el borrow checker).
#[derive(Default)]
struct Deferred {
    remove: Option<String>,
    reorder: Option<(usize, bool)>,
    expanded: Option<Option<String>>,
    net_tab: Option<usize>,
    set_setting: Option<(String, String, String, NetworkKind)>,
    set_name: Option<(String, String)>,
    set_enabled: Option<(String, bool)>,
    add_header: Option<(String, String, String)>,
    del_header: Option<(String, String)>,
    test_manifest: Option<String>,
    refresh: bool,
}

impl Deferred {
    fn apply(self, app: &mut App) {
        if self.refresh {
            let reg = app.svc.reg.clone();
            let tx = app.svc.atx.clone();
            app.svc.spawn(move || {
                let net = NetworkKind::detect();
                let msg = match reg.refresh_manifests(net) {
                    Ok(0) => "Manifests al día".to_string(),
                    Ok(n) => format!("{n} manifest(s) actualizado(s)"),
                    Err(e) => format!("Error refrescando: {e}"),
                };
                let _ = tx.send(crate::net::AddonResult::Toast { msg, error: false });
                let _ = tx.send(crate::net::AddonResult::ManifestChanged);
            });
        }
        if let Some((base, key, value, net)) = self.set_setting {
            app.svc.reg.set_setting(&base, &key, value, net);
        }
        if let Some((base, name)) = self.set_name {
            app.svc.reg.set_name(&base, name);
        }
        if let Some((base, on)) = self.set_enabled {
            app.svc.reg.set_enabled(&base, on);
        }
        if let Some((base, k, v)) = self.add_header {
            app.svc.reg.set_header(&base, &k, v);
        }
        if let Some((base, k)) = self.del_header {
            app.svc.reg.set_header(&base, &k, String::new());
        }
        if let Some(exp) = self.expanded {
            app.addons_view.expanded = exp;
        }
        if let Some(t) = self.net_tab {
            app.addons_view.net_tab = t;
        }
        if let Some((i, up)) = self.reorder {
            let order: Vec<String> = app.addons_sorted().iter().map(|a| a.base.clone()).collect();
            if i < order.len() {
                let j = if up {
                    i.saturating_sub(1)
                } else {
                    (i + 1).min(order.len() - 1)
                };
                if j != i {
                    let mut o = order;
                    o.swap(i, j);
                    app.svc.reg.reorder(o);
                }
            }
        }
        if let Some(base) = self.remove {
            app.svc.reg.remove(&base);
            app.addons_view.expanded = None;
            app.toast("Addon desinstalado", false);
            app.load_home();
        }
        if let Some(base) = self.test_manifest {
            let reg = app.svc.reg.clone();
            let tx = app.svc.atx.clone();
            app.svc.spawn(move || {
                let t = std::time::Instant::now();
                let (msg, error) = match reg.api.fetch_manifest(&base, std::time::Duration::ZERO) {
                    Ok(m) => (
                        format!(
                            "{} v{} respondió en {} · {} settings · {} catálogos",
                            m.display_name(),
                            m.version,
                            crate::util::fmt_latency(t.elapsed()),
                            m.settings.len(),
                            m.catalogs.len()
                        ),
                        false,
                    ),
                    Err(e) => (format!("manifest: {e}"), true),
                };
                let _ = tx.send(crate::net::AddonResult::Toast { msg, error });
            });
        }
    }
}

pub fn view(app: &mut App, ui: &mut Ui) {
    let mut d = Deferred::default();

    ui.horizontal(|ui| {
        widgets::page_title(ui, "Addons", None);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let n = app.svc.reg.count();
            ui.label(RichText::new(format!("{n} instalado(s)")).size(11.0).color(TEXT_FAINT));
            if widgets::pill(ui, "Refrescar manifests", false).clicked() {
                d.refresh = true;
            }
        });
    });

    // ── instalar ──
    ui.add_space(8.0);
    Frame::NONE
        .fill(CARD)
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.label(RichText::new("Instalar addon por URL").strong());
            ui.label(
                RichText::new(
                    "Pegá la URL del manifest.json (o la base). Los settings declarados se envían como query params en cada petición, igual que en Eclipse.",
                )
                .size(11.0)
                .color(TEXT_DIM),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let resp = ui.add_sized(
                    Vec2::new((ui.available_width() - 140.0).max(160.0), 30.0),
                    egui::TextEdit::singleline(&mut app.addons_view.url)
                        .hint_text("https://host/token/manifest.json")
                        .id(egui::Id::new("addon.url")),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let can = !app.addons_view.url.trim().is_empty() && !app.addons_view.installing;
                let b = egui::Button::new(RichText::new("  Instalar  ").color(Color32::WHITE))
                    .fill(if can { ACCENT } else { Color32::from_rgb(48, 52, 68) })
                    .corner_radius(CornerRadius::same(8))
                    .min_size(Vec2::new(110.0, 30.0));
                if ui.add_enabled(can, b).clicked() || enter {
                    let u = app.addons_view.url.clone();
                    app.install_addon(&u);
                }
                if app.addons_view.installing {
                    widgets::spinner(ui, 14.0);
                }
            });
        });

    ui.add_space(12.0);

    // ── lista ──
    let addons = app.addons_sorted();
    let expanded_base = app.addons_view.expanded.clone();
    let net_tab = app.addons_view.net_tab;
    let net_for_settings = if net_tab == 0 { NetworkKind::Wifi } else { NetworkKind::Cellular };
    let settings_now: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> =
        addons
            .iter()
            .map(|a| (a.base.clone(), app.settings_map(&a.base, net_for_settings)))
            .collect();
    let total = addons.len();

    ScrollArea::vertical()
        .auto_shrink(false)
        .id_salt("addons.scroll")
        .show(ui, |ui| {
            for (i, a) in addons.iter().enumerate() {
                let base = a.base.clone();
                let expanded = expanded_base.as_deref() == Some(base.as_str());
                let m = rg(&a.manifest).clone();
                let entry = rg(&a.entry).clone();

                Frame::NONE
                    .fill(if expanded { CARD_HOVER } else { CARD })
                    .stroke(Stroke::new(
                        1.0,
                        if entry.enabled {
                            BORDER
                        } else {
                            Color32::from_rgb(28, 30, 40)
                        },
                    ))
                    .corner_radius(CornerRadius::same(RADIUS))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            widgets::cover(ui, m.icon.as_deref(), 44.0, 8, &m.display_name());
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(m.display_name()).size(14.5).strong());
                                    widgets::badge(ui, &format!("v{}", m.version), TEXT_FAINT);
                                    if !entry.enabled {
                                        widgets::badge(ui, "desactivado", WARN);
                                    }
                                    if entry.priority == 0 {
                                        widgets::badge(ui, "prioridad 1", ACCENT_2).on_hover_text(
                                            "primer addon de la cadena de reproducción",
                                        );
                                    }
                                });
                                ui.label(
                                    RichText::new(crate::util::ellipsize(
                                        m.description.as_deref().unwrap_or(&m.id),
                                        120,
                                    ))
                                    .size(11.5)
                                    .color(TEXT_DIM),
                                );
                                ui.horizontal(|ui| {
                                    for r in &m.resources {
                                        widgets::badge(
                                            ui,
                                            r,
                                            match r.as_str() {
                                                "search" | "stream" => OK,
                                                "catalog" => ACCENT_2,
                                                "settings" => ACCENT,
                                                "isrc" | "resolve" => ACCENT_SOFT,
                                                _ => TEXT_FAINT,
                                            },
                                        );
                                    }
                                    ui.label(
                                        RichText::new(format!("· {}", m.types.join(", ")))
                                            .size(10.5)
                                            .color(TEXT_FAINT),
                                    );
                                });
                            });

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if widgets::icon_button(ui, "🗑", "Desinstalar", false).clicked() {
                                    d.remove = Some(base.clone());
                                }
                                if widgets::icon_button(ui, "↑", "Subir prioridad", i > 0).clicked() {
                                    d.reorder = Some((i, true));
                                }
                                if widgets::icon_button(
                                    ui,
                                    "↓",
                                    "Bajar prioridad",
                                    i + 1 < total,
                                )
                                .clicked() {
                                    d.reorder = Some((i, false));
                                }
                                let on = entry.enabled;
                                if widgets::pill(ui, if on { "Activo" } else { "Inactivo" }, on)
                                    .clicked()
                                {
                                    d.set_enabled = Some((base.clone(), !on));
                                }
                                if widgets::icon_button(
                                    ui,
                                    "⚙",
                                    if m.settings.is_empty() {
                                        "Sin settings declarados"
                                    } else {
                                        "Configurar addon"
                                    },
                                    expanded,
                                )
                                .clicked()
                                {
                                    d.expanded = Some(if expanded {
                                        None
                                    } else {
                                        Some(base.clone())
                                    });
                                }
                            });
                        });

                        if expanded {
                            ui.add_space(8.0);
                            ui.separator();
                            settings_panel(
                                ui,
                                &mut d,
                                &base,
                                &m.settings,
                                &entry.name_override,
                                &entry.headers,
                                net_tab,
                                settings_now.get(&base).cloned().unwrap_or_default(),
                            );
                        }
                    });
                ui.add_space(8.0);
            }

            if total == 0 {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("⧉").size(40.0).color(TEXT_FAINT));
                    ui.label(
                        RichText::new("Todavía no instalaste ningún addon")
                            .size(13.0)
                            .color(TEXT_DIM),
                    );
                    ui.label(
                        RichText::new(
                            "Un addon es un servidor HTTP que expone /search, /stream y, opcionalmente,\n/album, /artist, /playlist, /catalog, /resolve-isrc y /resolve.",
                        )
                        .size(11.5)
                        .color(TEXT_FAINT),
                    );
                });
            }
        });

    d.apply(app);
}

#[allow(clippy::too_many_arguments)]
fn settings_panel(
    ui: &mut Ui,
    d: &mut Deferred,
    base: &str,
    defs: &[SettingDef],
    name_override: &Option<String>,
    headers: &std::collections::BTreeMap<String, String>,
    net_tab: usize,
    values: std::collections::BTreeMap<String, String>,
) {
    let net = if net_tab == 0 {
        NetworkKind::Wifi
    } else {
        NetworkKind::Cellular
    };
    let detected = NetworkKind::detect();

    ui.horizontal(|ui| {
        ui.label(RichText::new("Red:").size(12.0).color(TEXT_DIM));
        if widgets::pill(ui, "Wi-Fi", net == NetworkKind::Wifi).clicked() {
            d.net_tab = Some(0);
        }
        if widgets::pill(ui, "Celular", net == NetworkKind::Cellular).clicked() {
            d.net_tab = Some(1);
        }
        ui.label(
            RichText::new(format!("(detectada: {})", detected.label()))
                .size(10.5)
                .color(TEXT_FAINT),
        )
        .on_hover_text(
            "Los settings con `perNetwork` guardan un valor distinto por red.\n\
             La app envía el que corresponde a la conexión actual.",
        );
    });
    ui.add_space(6.0);

    // nombre visible
    let mut custom = name_override.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Nombre visible").size(12.0).color(TEXT_DIM));
        let r = ui.add_sized(
            Vec2::new(220.0, 26.0),
            egui::TextEdit::singleline(&mut custom).hint_text("como lo querés ver"),
        );
        if r.lost_focus() {
            d.set_name = Some((base.to_string(), custom.clone()));
        }
        ui.label(
            RichText::new("(cosmético, no se envía al addon)")
                .size(10.5)
                .color(TEXT_FAINT),
        );
    });
    ui.add_space(8.0);

    if defs.is_empty() {
        ui.label(
            RichText::new("Este addon no declara settings.")
                .size(11.5)
                .color(TEXT_FAINT),
        );
    }

    for def in defs {
        let value = values.get(&def.key).cloned().unwrap_or_default();
        let mut changed: Option<String> = None;

        ui.horizontal(|ui| {
            ui.set_min_width(230.0);
            ui.label(RichText::new(def.label()).size(12.5).color(TEXT))
                .on_hover_text(def.help.clone().unwrap_or_default());
            if def.per_network {
                widgets::badge(ui, "perNetwork", ACCENT_SOFT);
            }
        });

        ui.horizontal(|ui| {
            ui.add_space(14.0);
            match def.kind {
                SettingType::Select => {
                    let sel_label = def
                        .options
                        .iter()
                        .find(|o| o.value == value)
                        .map(|o| o.label().to_string())
                        .unwrap_or_else(|| {
                            if value.is_empty() {
                                "— (default del addon)".into()
                            } else {
                                value.clone()
                            }
                        });
                    egui::ComboBox::from_id_salt(format!("set.{}.{}.{}", base, def.key, net_tab))
                        .selected_text(RichText::new(crate::util::ellipsize(&sel_label, 62)).size(12.0))
                        .width(460.0)
                        .show_ui(ui, |ui| {
                            for o in &def.options {
                                if ui
                                    .selectable_label(
                                        o.value == value,
                                        crate::util::ellipsize(o.label(), 76),
                                    )
                                    .clicked()
                                {
                                    changed = Some(o.value.clone());
                                }
                            }
                            if ui
                                .selectable_label(value.is_empty(), "— (dejar en blanco)")
                                .clicked()
                            {
                                changed = Some(String::new());
                            }
                        });
                }
                SettingType::Toggle => {
                    let on = if value.is_empty() {
                        def.default_bool()
                    } else {
                        value == "true"
                    };
                    if widgets::pill(ui, if on { "ON" } else { "OFF" }, on).clicked() {
                        changed = Some(if on { "false".into() } else { "true".into() });
                    }
                    ui.label(
                        RichText::new(if on { "true" } else { "false" })
                            .size(11.0)
                            .monospace()
                            .color(TEXT_FAINT),
                    );
                }
                SettingType::Number => {
                    let mut n: f64 = value
                        .parse()
                        .unwrap_or_else(|_| def.default_str().and_then(|v| v.parse().ok()).unwrap_or(0.0));
                    let mut dv = egui::DragValue::new(&mut n).speed(0.5);
                    if let Some(min) = def.min {
                        dv = dv.range(min..=def.max.unwrap_or(f64::MAX));
                    }
                    if ui.add(dv).changed() {
                        changed = Some(n.to_string());
                    }
                }
                _ => {
                    let mut txt = value.clone();
                    let mut te = egui::TextEdit::singleline(&mut txt).desired_width(360.0);
                    if let Some(ph) = &def.placeholder {
                        te = te.hint_text(ph.clone());
                    }
                    if ui.add(te).changed() {
                        changed = Some(txt.clone());
                    }
                    if let Some(max) = def.max_length {
                        ui.label(
                            RichText::new(format!("{}/{}", txt.chars().count(), max))
                                .size(10.0)
                                .color(TEXT_FAINT),
                        );
                    }
                }
            }
        });

        if let Some(help) = &def.help {
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(
                    RichText::new(crate::util::ellipsize(help, 160))
                        .size(10.5)
                        .color(TEXT_FAINT),
                );
            });
        }

        if let Some(v) = changed {
            d.set_setting = Some((base.to_string(), def.key.clone(), v, net));
        }
        ui.add_space(8.0);
    }

    // ── headers ──
    ui.separator();
    ui.label(RichText::new("Headers HTTP extra").size(12.0).strong());
    ui.label(
        RichText::new("Se mandan en todas las peticiones al addon (tokens, autorización, etc.).")
            .size(10.5)
            .color(TEXT_FAINT),
    );
    for k in headers.keys() {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            widgets::badge(ui, k, ACCENT_2);
            ui.label(
                RichText::new(crate::util::ellipsize(&headers[k], 60))
                    .size(11.0)
                    .monospace()
                    .color(TEXT_DIM),
            );
            if widgets::icon_button(ui, "✕", "Quitar header", false).clicked() {
                d.del_header = Some((base.to_string(), k.clone()));
            }
        });
    }
    ui.add_space(4.0);

    // ── base + prueba ──
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(RichText::new("base:").size(10.5).color(TEXT_FAINT));
        ui.label(RichText::new(base).size(10.5).monospace().color(TEXT_DIM));
        if widgets::icon_button(ui, "⌁", "Probar /manifest.json ahora", false).clicked() {
            d.test_manifest = Some(base.to_string());
        }
    });
}
