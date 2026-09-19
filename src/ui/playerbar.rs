//! Barra inferior de reproducción.

use egui::{Align, Color32, Layout, Rect, RichText, Sense, Stroke, Ui, Vec2};

use crate::player::Repeat;
use crate::ui::app::App;
use crate::ui::theme::*;
use crate::ui::widgets;

pub fn ui(app: &mut App, ui: &mut Ui) {
    let st = app.svc.player.state();
    let current = app.svc.player.queue.current();

    ui.horizontal(|ui| {
        // ── izquierda: portada + metadatos ──
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width().min(340.0).max(200.0), 72.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                let (art, title, sub) = match &current {
                    Some(q) => (
                        q.artwork.clone(),
                        q.title.clone(),
                        q.subtitle(),
                    ),
                    None => (None, "Nada en reproducción".to_string(), "elegí un tema para empezar".to_string()),
                };
                let fav = current
                    .as_ref()
                    .map(|q| app.is_favorite(&q.addon_id, &q.track_id))
                    .unwrap_or(false);
                widgets::cover(ui, art.as_deref(), 58.0, RADIUS, &title);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(crate::util::ellipsize(&title, 44))
                            .size(13.5)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        RichText::new(crate::util::ellipsize(&sub, 52))
                            .size(11.5)
                            .color(TEXT_DIM),
                    );
                    ui.horizontal(|ui| {
                        if let Some(a) = &st.audio {
                            let b = a.badge();
                            if !b.is_empty() {
                                widgets::badge(ui, &b, if a.bit_depth >= 24 { OK } else { ACCENT_2 });
                            }
                        }
                        if let Some(via) = &app.diag.resolved_via {
                            widgets::badge(ui, via, TEXT_FAINT).on_hover_text("addon y método de resolución del stream");
                        }
                        if let Some(ms) = app.diag.resolve_ms {
                            widgets::badge(ui, &format!("{ms}ms"), ACCENT_SOFT)
                                .on_hover_text("cuánto tardó en resolverse la URL de audio");
                        }
                        if app.diag.prerolled {
                            widgets::badge(ui, "gapless", OK);
                        }
                    });
                });
                if widgets::icon_button(ui, if fav { "♥" } else { "♡" }, "Favorito (F)", fav).clicked() {
                    app.toggle_favorite_current();
                }
            },
        );

        // ── centro: transporte + seek ──
        let center_w = (ui.available_width() - 300.0).max(220.0);
        ui.allocate_ui_with_layout(
            Vec2::new(center_w, 72.0),
            Layout::top_down(Align::Center),
            |ui| {
                ui.horizontal(|ui| {
                    let rep = app.svc.player.queue.repeat().clone();
                    let shuf = app.svc.prefs().shuffle;
                    if widgets::icon_button(ui, "⇄", "Aleatorio (S)", shuf).clicked() {
                        let s = !shuf;
                        app.svc.set_prefs(|p| p.shuffle = s);
                        app.svc.player.queue.set_shuffle(s);
                    }
                    if widgets::icon_button_sized(ui, "⏮", "Anterior (⌘←)", false, 13.0).clicked() {
                        app.prev_track();
                    }
                    let glyph = if st.paused || !st.playing { "▶" } else { "⏸" };
                    let play_resp = widgets::icon_button_sized(
                        ui,
                        glyph,
                        if st.playing && !st.paused {
                            "Pausar (espacio)"
                        } else {
                            "Reproducir (espacio)"
                        },
                        st.playing && !st.paused,
                        17.0,
                    );
                    if play_resp.clicked() {
                        if current.is_none() {
                            app.toast("No hay nada en la cola", true);
                        } else if st.slot.is_none() {
                            app.play_current(st.position.max(0.0));
                        } else {
                            app.svc.player.toggle_pause();
                        }
                    }
                    if widgets::icon_button_sized(ui, "⏭", "Siguiente (⌘→)", false, 13.0).clicked() {
                        app.next_track();
                    }
                    let tip = match rep {
                        Repeat::Off => "Repetir: no",
                        Repeat::All => "Repetir: toda la cola",
                        Repeat::One => "Repetir: este tema",
                    };
                    if widgets::icon_button(ui, rep.icon(), &format!("{tip} (R)"), rep != Repeat::Off)
                        .clicked()
                    {
                        let r = rep.next();
                        let rs = r.as_str().to_string();
                        app.svc.player.queue.set_repeat(r);
                        app.svc.set_prefs(move |p| p.repeat = rs);
                    }
                });

                ui.add_space(2.0);

                // seek
                let dur: f64 = if st.duration > 0.0 {
                    st.duration
                } else {
                    current
                        .as_ref()
                        .and_then(|q| q.duration)
                        .unwrap_or(0.0)
                        .max(st.position)
                };
                let pos = st.position.min(if dur > 0.0 { dur } else { st.position });
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::util::fmt_duration(pos))
                            .size(10.5)
                            .monospace()
                            .color(TEXT_FAINT),
                    );
                    let w = (ui.available_width() - 46.0).max(80.0);
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(w, 14.0), Sense::click_and_drag());
                    let bar = Rect::from_center_size(
                        rect.center(),
                        Vec2::new(rect.width(), 4.0),
                    );
                    let frac: f32 = if dur > 0.0 {
                        (pos / dur) as f32
                    } else {
                        0.0
                    };
                    let inter = ui.interact(
                        rect,
                        ui.id().with("seek"),
                        Sense::click_and_drag(),
                    );
                    let hovered = inter.hovered() || inter.dragged();
                    let mut new_frac = frac;
                    if inter.dragged() || inter.clicked() {
                        if let Some(p) = inter.interact_pointer_pos() {
                            new_frac = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                        }
                    }
                    let buffered = st.buffering.map(|b| b.clamp(0.0, 1.0) as f32);
                    crate::ui::shell::thin_bar(
                        ui,
                        bar,
                        if inter.dragged() { new_frac } else { frac },
                        buffered,
                        if hovered { ACCENT } else { ACCENT_SOFT },
                    );
                    if (inter.changed() || inter.clicked()) && dur > 0.0 {
                        app.svc.player.seek(new_frac as f64 * dur);
                    }
                    ui.label(
                        RichText::new(crate::util::fmt_duration(dur))
                            .size(10.5)
                            .monospace()
                            .color(TEXT_FAINT),
                    );
                });
            },
        );

        // ── derecha: volumen + cola + info ──
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let queue_n = app.svc.player.queue.len();
            let cur_idx = app.svc.player.queue.cursor();
            if widgets::icon_button(
                ui,
                "☰",
                &format!("Cola ({queue_n}) — Q"),
                app.show_queue,
            )
            .clicked()
            {
                app.show_queue = !app.show_queue;
            }
            if queue_n > 0 {
                ui.label(
                    RichText::new(format!("{}/{}", cur_idx + 1, queue_n))
                        .size(11.0)
                        .monospace()
                        .color(TEXT_FAINT),
                )
                .on_hover_text("posición en la cola");
            }

            // volumen
            let vol = st.volume.clamp(0.0, 1.5);
            let muted = st.muted;
            let glyph = if muted || vol <= 0.001 {
                "✕"
            } else if vol < 0.45 {
                "◂"
            } else {
                "◂◂"
            };
            if widgets::icon_button(ui, glyph, "Silencio (M)", muted).clicked() {
                let m = !muted;
                app.svc.player.set_muted(m);
                app.svc.set_prefs(move |p| p.muted = m);
            }
            let (rect, _) = ui.allocate_exact_size(Vec2::new(96.0, 16.0), Sense::click_and_drag());
            let bar = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 4.0));
            let inter = ui.interact(rect, ui.id().with("vol"), Sense::click_and_drag());
            let mut nv = vol;
            if inter.dragged() || inter.clicked() {
                if let Some(p) = inter.interact_pointer_pos() {
                    nv = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0) * 1.5;
                }
            }
            crate::ui::shell::thin_bar(
                ui,
                bar,
                if muted { 0.0 } else { nv / 1.5 },
                None,
                ACCENT_2,
            );
            if inter.dragged() || inter.changed() || inter.clicked() {
                app.svc.player.set_volume(nv);
                app.svc.player.set_muted(false);
                let v = nv;
                app.svc.set_prefs(move |p| {
                    p.volume = v;
                    p.muted = false;
                });
            }
            ui.label(
                RichText::new(format!("{:.0}%", (if muted { 0.0 } else { nv }) * 100.0 / 1.5))
                    .size(10.5)
                    .monospace()
                    .color(TEXT_FAINT),
            )
            .on_hover_text("volumen (0–150%)");

            // gap medido en la última transición
            if let Some(g) = app.diag.last_gap_ms {
                let color = if g <= 60 { OK } else if g <= 250 { WARN } else { DANGER };
                widgets::badge(ui, &format!("gap {g}ms"), color).on_hover_text(format!(
                    "silencio entre el fin de un tema y el arranque del siguiente\n\
                     promedio: {} ms sobre {} transiciones",
                    app.diag.avg_gap().map(|v| format!("{v:.0}")).unwrap_or("—".into()),
                    app.diag.gaps.len()
                ));
            }
            if let Some(b) = st.buffering {
                if b < 0.999 {
                    widgets::badge(ui, &format!("buffer {:.0}%", b * 100.0), WARN);
                }
            }
        });
    });

    // borde superior sutil
    let r = ui.max_rect();
    ui.painter().line_segment(
        [
            egui::pos2(r.left(), r.top() - 6.0),
            egui::pos2(r.right(), r.top() - 6.0),
        ],
        Stroke::new(1.0, Color32::from_rgb(28, 31, 42)),
    );

}
