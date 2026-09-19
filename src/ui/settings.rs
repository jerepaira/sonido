//! Ajustes de la aplicación.

use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Stroke, Ui, Vec2};

use crate::ui::app::App;
use crate::ui::theme::*;
use crate::ui::widgets;

const TABS: [&str; 4] = ["Reproducción", "Audio", "Red y caché", "Acerca de"];

pub fn view(app: &mut App, ui: &mut Ui) {
    widgets::page_title(ui, "Ajustes", None);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        for (i, t) in TABS.iter().enumerate() {
            if widgets::pill(ui, t, app.settings_tab == i).clicked() {
                app.settings_tab = i;
            }
        }
    });
    ui.add_space(12.0);

    ScrollArea::vertical()
        .auto_shrink(false)
        .id_salt("settings.scroll")
        .show(ui, |ui| match app.settings_tab {
            0 => playback(app, ui),
            1 => audio(app, ui),
            2 => network(app, ui),
            _ => about(app, ui),
        });
}

fn section(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    Frame::NONE
        .fill(CARD)
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(title).text_style(ts::subtitle()).strong());
            ui.add_space(6.0);
            body(ui);
        });
    ui.add_space(10.0);
}

fn row(ui: &mut Ui, label: &str, help: &str, w: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(250.0, 24.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.label(RichText::new(label).size(12.5).color(TEXT)).on_hover_text(help);
            },
        );
        w(ui);
    });
    if !help.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(250.0);
            ui.label(RichText::new(help).size(10.5).color(TEXT_FAINT));
        });
    }
    ui.add_space(6.0);
}

fn toggle(ui: &mut Ui, on: bool) -> Option<bool> {
    let mut out = None;
    if widgets::pill(ui, if on { "ON" } else { "OFF" }, on).clicked() {
        out = Some(!on);
    }
    out
}

// ─────────────────────────── reproducción ───────────────────────────

fn playback(app: &mut App, ui: &mut Ui) {
    let p = app.svc.prefs();

    section(ui, "Búsqueda", |ui| {
        row(
            ui,
            "Buscar en todos los addons",
            "ON: consulta en paralelo a todos los addons habilitados y muestra cada respuesta apenas llega.\nOFF: usa solo el addon activo del selector del sidebar.",
            |ui| {
                if let Some(v) = toggle(ui, p.search_all_addons) {
                    app.svc.set_prefs(|x| x.search_all_addons = v);
                }
            },
        );
        row(
            ui,
            "Rellenar portadas por ISRC",
            "Muchos addons mandan los metadatos pero no la portada (o una muy chica).\nCon esto ON, la app busca la portada que falta en la API pública de iTunes\n(usando el ISRC del tema) y la cachea 7 días. Solo consulta lo que falta.",
            |ui| {
                if let Some(v) = toggle(ui, p.artwork_lookup) {
                    app.svc.set_prefs(|x| x.artwork_lookup = v);
                    app.toast(
                        if v {
                            "Relleno de portadas activado"
                        } else {
                            "Relleno de portadas desactivado"
                        },
                        false,
                    );
                }
            },
        );
        row(
            ui,
            "Ocultar contenido explícito",
            "Filtra los temas/álbumes marcados `explicit` por el addon.",
            |ui| {
                if let Some(v) = toggle(ui, p.hide_explicit) {
                    app.svc.set_prefs(|x| x.hide_explicit = v);
                }
            },
        );
    });

    section(ui, "Cola y continuidad", |ui| {
        row(
            ui,
            "Reproducción gapless",
            "Pre-carga el siguiente tema en el motor de audio para que no haya corte entre pistas.",
            |ui| {
                if let Some(v) = toggle(ui, p.gapless) {
                    app.svc.set_prefs(|x| x.gapless = v);
                }
            },
        );
        row(
            ui,
            "Resolver el siguiente con anticipación",
            "Pide la URL del próximo tema antes de que termine el actual (necesita gapless).",
            |ui| {
                if let Some(v) = toggle(ui, p.crossfade_prefetch) {
                    app.svc.set_prefs(|x| x.crossfade_prefetch = v);
                }
            },
        );
        row(
            ui,
            "Verificar la URL antes de reproducir",
            "Hace un HEAD/Range de 1 KB a la URL de audio antes de mandarla al motor.\nCuesta un round-trip chico, pero convierte \"no suena\" en un error concreto\n(403 = token expirado, 404 = no existe, timeout = CDN caído).",
            |ui| {
                if let Some(v) = toggle(ui, p.probe_stream) {
                    app.svc.set_prefs(|x| x.probe_stream = v);
                }
            },
        );
        row(
            ui,
            "Anticipación (s)",
            "Cuántos segundos antes del final se resuelve y pre-carga el siguiente tema.",
            |ui| {
                let mut v = p.preroll_secs;
                if ui
                    .add(egui::DragValue::new(&mut v).speed(0.5).range(2.0..=120.0).suffix(" s"))
                    .changed()
                {
                    app.svc.set_prefs(move |x| x.preroll_secs = v.clamp(2.0, 120.0));
                }
            },
        );
        row(
            ui,
            "Aleatorio por defecto",
            "Estado inicial del shuffle al arrancar.",
            |ui| {
                if let Some(v) = toggle(ui, p.shuffle) {
                    app.svc.set_prefs(|x| x.shuffle = v);
                    app.svc.player.queue.set_shuffle(v);
                }
            },
        );
    });

    section(ui, "Preferencia de calidad al elegir stream", |ui| {
        row(
            ui,
            "Preferir Hi-Res",
            "Da más puntaje a FLAC 24-bit / altas frecuencias de muestreo.",
            |ui| {
                if let Some(v) = toggle(ui, p.prefer_hires) {
                    app.svc.set_prefs(|x| x.prefer_hires = v);
                }
            },
        );
        row(
            ui,
            "Preferir Dolby Atmos",
            "Da más puntaje a E-AC-3 J-OC / AC-4. El downmix a tu dispositivo lo hace el motor.",
            |ui| {
                if let Some(v) = toggle(ui, p.prefer_atmos) {
                    app.svc.set_prefs(|x| x.prefer_atmos = v);
                }
            },
        );
        ui.label(
            RichText::new(
                "Nota: la calidad real la define cada addon (ver sus settings en la pestaña Addons).",
            )
            .size(10.5)
            .color(TEXT_FAINT),
        );
    });
}

// ─────────────────────────── audio ───────────────────────────

fn audio(app: &mut App, ui: &mut Ui) {
    let p = app.svc.prefs();
    let st = app.svc.player.state();

    section(ui, "Motor", |ui| {
        row(ui, "Backend activo", "libmpv = gapless real, FLAC Hi-Res y Dolby Atmos.\nrodio = fallback 100% Rust si no está libmpv.", |ui| {
            widgets::badge(ui, &st.backend, if st.backend == "libmpv" { OK } else { WARN });
        });
        row(ui, "Estado", "", |ui| {
            ui.label(RichText::new(app.svc.player.backend.backend_stats()).size(11.0).monospace().color(TEXT_DIM));
        });
        if st.backend != "libmpv" {
            ui.label(
                RichText::new("Instalá libmpv para gapless + Atmos:  sudo apt install libmpv2  (o mpv)")
                    .size(11.0)
                    .color(WARN),
            );
        }
        row(
            ui,
            "ReplayGain",
            "Normaliza el volumen usando las etiquetas del archivo (solo con libmpv).",
            |ui| {
                egui::ComboBox::from_id_salt("rg.mode")
                    .selected_text(match p.replaygain.as_str() {
                        "track" => "por tema",
                        "album" => "por álbum",
                        _ => "no",
                    })
                    .show_ui(ui, |ui| {
                        for (v, l) in [("off", "no"), ("track", "por tema"), ("album", "por álbum")] {
                            if ui.selectable_label(p.replaygain == v, l).clicked() {
                                let vv = v.to_string();
                                app.svc.set_prefs(move |x| x.replaygain = vv.clone());
                                app.svc.player.set_replaygain(v);
                            }
                        }
                    });
            },
        );
    });

    section(ui, "Salida", |ui| {
        let devices = app.svc.player.backend.audio_devices();
        row(ui, "Dispositivo", "Requiere libmpv. `auto` usa el dispositivo por defecto del sistema.", |ui| {
            let cur = p.audio_device.clone().unwrap_or_else(|| "auto".into());
            egui::ComboBox::from_id_salt("audio.dev")
                .selected_text(RichText::new(crate::util::ellipsize(&cur, 40)).size(12.0))
                .width(380.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(cur == "auto", "auto (por defecto)").clicked() {
                        app.svc.set_prefs(|x| x.audio_device = None);
                        app.svc.player.backend.set_audio_device("auto");
                    }
                    for (id, desc) in &devices {
                        if ui
                            .selectable_label(&cur == id, crate::util::ellipsize(desc, 50))
                            .clicked()
                        {
                            let id2 = id.clone();
                            let id3 = id.clone();
                            app.svc.set_prefs(move |x| x.audio_device = Some(id2));
                            app.svc.player.backend.set_audio_device(&id3);
                        }
                    }
                });
        });
        row(
            ui,
            "Modo exclusivo (bit-perfect)",
            "Toma el dispositivo en exclusiva y evita el resampling del mezclador del sistema.\nPuede fallar si otro programa lo está usando.",
            |ui| {
                if let Some(v) = toggle(ui, p.audio_exclusive) {
                    app.svc.set_prefs(|x| x.audio_exclusive = v);
                    app.svc.player.backend.set_exclusive(v);
                }
            },
        );
        row(ui, "Volumen", "0–150%. Por encima de 100% puede recortar.", |ui| {
            let mut v = p.volume;
            if ui
                .add(egui::Slider::new(&mut v, 0.0..=1.5).text("vol"))
                .changed()
            {
                app.svc.set_prefs(|x| x.volume = v);
                app.svc.player.set_volume(v);
            }
        });
        row(
            ui,
            "Velocidad",
            "Útil para podcasts/audiolibros.",
            |ui| {
                let mut v = p.speed;
                if ui
                    .add(egui::Slider::new(&mut v, 0.5..=2.0).text("x"))
                    .changed()
                {
                    app.svc.set_prefs(|x| x.speed = v);
                    app.svc.player.set_speed(v);
                }
            },
        );
    });
}

// ─────────────────────────── red y caché ───────────────────────────

fn network(app: &mut App, ui: &mut Ui) {
    let p = app.svc.prefs();
    let net = app.svc.net();
    let (reqs, errs, hits, bytes) = app.cache_stats();

    section(ui, "Conexión", |ui| {
        row(ui, "Red detectada", "Se usa para elegir el set de settings `perNetwork` del addon.", |ui| {
            widgets::badge(ui, net.label(), if net == crate::net::NetworkKind::Wifi { OK } else { WARN });
        });
        ui.label(
            RichText::new("Podés forzarla con la variable de entorno SONIDO_NETWORK=wifi|cellular")
                .size(10.5)
                .color(TEXT_FAINT),
        );
    });

    section(ui, "Caché", |ui| {
        row(ui, "Peticiones HTTP", "", |ui| {
            ui.label(RichText::new(format!("{reqs} · {errs} fallidas · {hits} hits de caché")).size(11.5).monospace().color(TEXT_DIM));
        });
        row(ui, "Descargado", "", |ui| {
            ui.label(RichText::new(crate::util::fmt_bytes(bytes)).size(11.5).monospace().color(TEXT_DIM));
        });
        row(ui, "Caché de HTTP en disco", "", |ui| {
            ui.label(RichText::new(crate::util::fmt_bytes(app.disk_cache_size())).size(11.5).monospace().color(TEXT_DIM));
        });
        row(
            ui,
            "Límite del caché",
            "Se poda lo más viejo al arrancar.",
            |ui| {
                let mut mb = p.cache_mb;
                if ui.add(egui::DragValue::new(&mut mb).speed(8).range(32..=8192).suffix(" MB")).changed() {
                    app.svc.set_prefs(move |x| x.cache_mb = mb.clamp(32, 8192));
                }
            },
        );
        row(
            ui,
            "TTL de manifests",
            "Cuánto se cachea el manifest.json en disco.",
            |ui| {
                let mut h = (p.manifest_ttl_secs / 3600).max(1);
                if ui.add(egui::DragValue::new(&mut h).speed(0.2).range(1..=168).suffix(" h")).changed() {
                    app.svc.set_prefs(move |x| x.manifest_ttl_secs = h.clamp(1, 168) * 3600);
                }
            },
        );
        row(
            ui,
            "TTL de álbum/artista/playlist",
            "Los resultados de búsqueda se cachean 30 min; los streams no se cachean a disco (las URLs expiran).",
            |ui| {
                let mut d = (p.metadata_ttl_secs / 86400).max(1);
                if ui.add(egui::DragValue::new(&mut d).speed(0.1).range(1..=60).suffix(" días")).changed() {
                    app.svc.set_prefs(move |x| x.metadata_ttl_secs = d.clamp(1, 60) * 86400);
                }
            },
        );
        ui.horizontal(|ui| {
            if widgets::pill(ui, "Vaciar caché HTTP", false).clicked() {
                app.svc.http.cache.clear();
                app.svc.resolver.cache.clear();
                app.toast("Caché HTTP vaciado", false);
            }
            if widgets::pill(ui, "Vaciar imágenes", false).clicked() {
                app.svc.image_loader.forget_all_egui(ui.ctx());
                app.toast("Caché de imágenes vaciado", false);
            }
            if widgets::pill(ui, "Podar ahora", false).clicked() {
                let max = p.cache_mb * 1024 * 1024;
                let c = app.svc.http.cache.clone();
                app.svc.spawn(move || c.prune(max));
                app.toast("Podando…", false);
            }
        });
    });

    section(ui, "Interfaz", |ui| {
        row(ui, "Mostrar FPS y latencias", "", |ui| {
            if let Some(v) = toggle(ui, p.show_fps) {
                app.svc.set_prefs(|x| x.show_fps = v);
            }
        });
        ui.add_space(6.0);
        if widgets::pill(ui, "Abrir ventana de depuración", false).clicked() {
            app.show_debug_window = true;
        }
    });
}

// ─────────────────────────── acerca de ───────────────────────────

fn about(app: &mut App, ui: &mut Ui) {
    section(ui, "Sonido", |ui| {
        ui.label(RichText::new(format!("versión {}", env!("CARGO_PKG_VERSION"))).size(12.0).color(TEXT_DIM));
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Reproductor de música nativo para Linux que consume addons con el protocolo de Eclipse Music:\n\
                 /manifest.json · /search · /stream · /album · /artist · /playlist · /catalog · /resolve-isrc · /resolve",
            )
            .size(11.5)
            .color(TEXT_FAINT),
        );
        ui.add_space(8.0);
        let line = |ui: &mut Ui, k: &str, v: String| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(k).size(11.5).color(TEXT_DIM));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(v).size(11.5).monospace().color(TEXT));
                });
            });
        };
        line(ui, "config", app.svc.store.config_path().display().to_string());
        line(ui, "biblioteca", app.svc.store.library_path().display().to_string());
        line(ui, "caché", app.svc.http.cache.root().display().to_string());
        line(ui, "motor", app.svc.player.state().backend);
        line(ui, "addons", app.svc.reg.count().to_string());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if widgets::pill(ui, "Guardar ahora", false).clicked() {
                app.svc.store.save();
                app.toast("Configuración guardada", false);
            }
            if widgets::pill(ui, "Abrir carpeta de config", false).clicked() {
                let path = crate::config::config_dir();
                let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
            }
        });
    });

    section(ui, "Atajos de teclado", |ui| {
        let keys = [
            ("Espacio", "play / pausa"),
            ("⌘/Ctrl + →", "tema siguiente"),
            ("⌘/Ctrl + ←", "tema anterior"),
            ("Shift + →", "+10 s"),
            ("Shift + ←", "−10 s"),
            ("/  o  ⌘K", "ir a buscar"),
            ("F", "favorito"),
            ("M", "silencio"),
            ("S", "aleatorio"),
            ("R", "repetir (off → toda → uno)"),
            ("Q", "panel de cola"),
            ("1 / 2 / 3", "Inicio / Buscar / Addons"),
            ("Esc", "volver"),
            ("Enter (en una fila)", "reproducir desde ahí"),
            ("Doble click", "reproducir la lista completa"),
        ];
        for (k, v) in keys {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(140.0, 20.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new(k).size(11.5).monospace().color(ACCENT_2));
                });
                ui.label(RichText::new(v).size(11.5).color(TEXT_DIM));
            });
        }
    });

    ui.add_space(6.0);
    Frame::NONE
        .fill(Color32::from_rgb(18, 22, 30))
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.label(RichText::new("Cómo funciona la resolución de un stream").strong());
            ui.label(
                RichText::new(
                    "1. Si el tema trae `streamURL`, se usa directo (cero round-trips).\n\
                     2. Si no, GET /stream/{id} al addon de origen, con sus settings como query params.\n\
                     3. Si eso falla, se recorre la cadena de addons por prioridad:\n\
                     ·      /resolve-isrc (si el tema tiene ISRC)\n\
                     ·      /resolve por identidad (isrc, título, artista, duración)\n\
                     ·      /search + scoring (ISRC > título > artista > duración), umbral 0.62\n\
                     4. Las URLs resueltas se cachean 4 min en memoria (o hasta su `expiresAt`).",
                )
                .size(11.0)
                .color(TEXT_DIM),
            );
        });
}
