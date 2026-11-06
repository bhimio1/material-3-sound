//! Effects: create equalizers and mix buses, switch them on and off, and
//! shape an equalizer on its frequency response (drag the band handles,
//! scroll on one for its width, double-click to add a band).

use crate::app::{status_banner, Action, View};
use crate::effects::{freq_label, freq_t, log_freq, Band, BandKind, Effect, EffectKind, Equalizer, PRESETS};
use crate::ui::kit;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Frame, Id, Margin, Mesh, Rect, RichText, Sense, Shape, Stroke, Ui};
use material_3_ui::common::{self, row};
use material_3_ui::icons::icons::*;
use material_3_ui::widgets::{self, ButtonKind};

#[derive(Default)]
pub struct State {
    pub open: Option<String>,
    shown: bool,
    band: usize,
    renaming: Option<(String, String)>,
    confirm_delete: Option<String>,
}

const MAX_BANDS: usize = 12;

pub fn show(ui: &mut Ui, view: &View, st: &mut State, out: &mut Vec<Action>) {
    if !st.shown {
        // Open the first equalizer, so there's something to shape.
        st.shown = true;
        if st.open.is_none() {
            st.open = view.cfg.effects.iter().find(|e| matches!(e.kind, EffectKind::Equalizer(_))).map(|e| e.id.clone());
        }
    }
    common::page(ui, ICON_EQUALIZER, "Effects", "Equalizers and mix buses that you patch between apps and devices.", |ui| {
        status_banner(ui, view, out);
        ui.add_space(18.0);
        let gap = 12.0;
        let w = ((ui.available_width() - gap) / 2.0).floor();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            if widgets::action_tile(ui, ICON_EQUALIZER, "New equalizer", "Shape the sound of whatever you patch into it", w, true).clicked() {
                out.push(Action::AddEqualizer);
            }
            if widgets::action_tile(ui, ICON_SPEAKER_GROUP, "New mix bus", "Mix several apps, then send them on together", w, false).clicked() {
                out.push(Action::AddBus);
            }
        });
        if view.cfg.effects.is_empty() {
            ui.add_space(20.0);
            common::empty_hint(ui, "No effects yet.");
        }
        for e in &view.cfg.effects {
            effect_card(ui, view, st, e, out);
        }
    });
    if let Some(id) = st.confirm_delete.clone() {
        let p = widgets::palette(ui);
        let name = view.cfg.effects.iter().find(|e| e.id == id).map(|e| e.name.clone()).unwrap_or_default();
        let mut close = false;
        common::modal(ui.ctx(), &p, "delete-effect", ICON_DELETE, &format!("Delete “{name}”?"), |ui| {
            ui.label(RichText::new("It stops now, and apps patched into it go quiet until you reconnect them.").color(p.on_surface_variant));
            common::dialog_buttons(ui, |ui| {
                if widgets::button(ui, ButtonKind::Danger, None, "Delete").clicked() {
                    out.push(Action::DeleteEffect(id.clone()));
                    close = true;
                }
                if widgets::button(ui, ButtonKind::Text, None, "Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            st.confirm_delete = None;
        }
    }
}

fn effect_card(ui: &mut Ui, view: &View, st: &mut State, e: &Effect, out: &mut Vec<Action>) {
    let p = widgets::palette(ui);
    let running = view.effect_running(e);
    let failed = view.failed.get(&e.id).copied().unwrap_or(0) >= 3;
    let (status, status_bg, status_fg) = if running {
        ("Running", p.primary_container, p.on_primary_container)
    } else if failed {
        ("Couldn't start", p.error_container, p.on_error_container)
    } else if e.enabled && view.starting.contains_key(&e.id) {
        ("Starting…", p.tertiary_container, p.on_tertiary_container)
    } else if e.enabled && view.cfg.restore_on_start {
        ("Waiting", p.surface_container_highest, p.on_surface_variant)
    } else {
        ("Off", p.surface_container_highest, p.on_surface_variant)
    };
    let open = st.open.as_deref() == Some(e.id.as_str());
    ui.add_space(16.0);
    Frame::new().fill(p.surface_container).corner_radius(CornerRadius::same(20)).inner_margin(Margin::symmetric(20, 14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
            let icon = if matches!(e.kind, EffectKind::Equalizer(_)) { ICON_EQUALIZER } else { ICON_SPEAKER_GROUP };
            kit::icon_badge(ui, rect.center(), 20.0, icon, p.secondary_container, p.on_secondary_container);
            ui.vertical(|ui| {
                match &mut st.renaming {
                    Some((id, name)) if *id == e.id => {
                        let r = widgets::field(ui, name, "Name", 220.0, Id::unique(("rename-effect", &e.id)));
                        r.request_focus();
                        if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !name.trim().is_empty() {
                            let mut new = e.clone();
                            new.name = name.trim().to_string();
                            out.push(Action::UpdateEffect(new));
                            st.renaming = None;
                        } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            st.renaming = None;
                        }
                    }
                    _ => {
                        ui.label(widgets::title_text(&e.name, 17.0).color(p.on_surface));
                    }
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(e.kind_label()).size(12.5).color(p.on_surface_variant));
                    kit::pill(ui, status, status_bg, status_fg);
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut on = e.enabled;
                if widgets::switch(ui, &mut on).on_hover_text(if on { "Stop" } else { "Start" }).clicked() {
                    out.push(Action::EnableEffect(e.id.clone(), on));
                }
                if widgets::icon_button(ui, ICON_DELETE, "Delete", true).clicked() {
                    st.confirm_delete = Some(e.id.clone());
                }
                if widgets::icon_button(ui, ICON_EDIT, "Rename", true).clicked() {
                    st.renaming = Some((e.id.clone(), e.name.clone()));
                }
                if running && widgets::icon_button(ui, ICON_CABLE, "Show in the patchbay", true).clicked() {
                    out.push(Action::Inspect(format!("node:{}", e.node_name())));
                }
                if matches!(e.kind, EffectKind::Equalizer(_)) {
                    let label = if open { "Done" } else { "Edit" };
                    if widgets::button(ui, ButtonKind::Tonal, Some(ICON_TUNE), label).clicked() {
                        st.open = if open { None } else { Some(e.id.clone()) };
                        st.band = 0;
                    }
                }
            });
        });
        if failed {
            let log = crate::config::effects_dir().join(format!("{}.log", e.id));
            common::note(ui, &format!("PipeWire wouldn't run it. Its log is in {}. Switch it off and on to try again.", material_3_ui::sys::paths::tilde(&log)));
        }
        let routing = routing_text(view, e);
        if !routing.is_empty() {
            common::note(ui, &routing);
        }
        if let (EffectKind::Equalizer(eq), true) = (&e.kind, open) {
            eq_editor(ui, view, st, e, eq, out);
        }
    });
}

/// "Spotify plays into it; it plays on Headphones."
fn routing_text(view: &View, e: &Effect) -> String {
    let Some((i, _)) = view.unit(&format!("node:{}", e.node_name())) else {
        return match e.kind {
            EffectKind::Bus(_) => "A virtual output: send apps into it in the patchbay, then send its monitor on to devices or recorders. Its volume sets the whole mix.".into(),
            EffectKind::Equalizer(_) => "Once it runs, patch apps into it and it into a device.".into(),
        };
    };
    let names = |v: Vec<usize>| v.into_iter().map(|j| view.units[j].title.clone()).collect::<Vec<_>>();
    let ins = names(view.sources_of(i));
    let outs = names(view.sends_of(i));
    let list = |v: &[String]| match v {
        [] => String::new(),
        [a] => a.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    let feeding = if ins.is_empty() { "Nothing plays into it yet".to_string() } else { format!("{} {} into it", list(&ins), if ins.len() == 1 { "plays" } else { "play" }) };
    let going = if outs.is_empty() { "it isn't connected to anything".to_string() } else { format!("it plays on {}", list(&outs)) };
    format!("{feeding}; {going}.")
}

fn db_slider(ui: &mut Ui, value: &mut f32, lo: f32, hi: f32, width: f32, color: Color32) -> bool {
    let mut t = *value - lo;
    let changed = kit::slider(ui, &mut t, hi - lo, width, color, Some(-lo)).changed();
    if changed {
        *value = ((t + lo) * 10.0).round() / 10.0;
    }
    changed
}

fn log_slider(ui: &mut Ui, value: &mut f32, lo: f32, hi: f32, width: f32, color: Color32) -> bool {
    let mut t = (*value / lo).ln() / (hi / lo).ln();
    let changed = kit::slider(ui, &mut t, 1.0, width, color, None).changed();
    if changed {
        *value = lo * (hi / lo).powf(t.clamp(0.0, 1.0));
    }
    changed
}

fn eq_editor(ui: &mut Ui, view: &View, st: &mut State, e: &Effect, eq: &Equalizer, out: &mut Vec<Action>) {
    let p = widgets::palette(ui);
    let mut eq = eq.clone();
    let mut changed = false;
    let rate = view.graph.server.as_ref().and_then(|s| s.rate).unwrap_or(48000) as f32;
    st.band = st.band.min(eq.bands.len().saturating_sub(1));

    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        ui.label(RichText::new("Presets").size(13.0).color(p.on_surface_variant));
        for (name, ..) in PRESETS {
            let current = Equalizer::preset(name).is_some_and(|pr| pr.bands == eq.bands && pr.preamp == eq.preamp);
            if widgets::filter_chip(ui, name, current, None).clicked() {
                if let Some(mut pr) = Equalizer::preset(name) {
                    pr.channels = eq.channels;
                    eq = pr;
                    changed = true;
                }
            }
        }
    });
    ui.add_space(10.0);
    changed |= response_graph(ui, &mut eq, &mut st.band, rate, Id::unique(("eq-graph", &e.id)));
    ui.add_space(6.0);

    row(ui, "Preamp", "Turn the whole signal down to make room for boosts, so loud parts don't clip.", |ui| {
        if widgets::button(ui, ButtonKind::Text, Some(ICON_AUTO_AWESOME), "Auto").on_hover_text("Just enough headroom for the loudest boost").clicked() {
            eq.preamp = eq.auto_preamp();
            changed = true;
        }
        ui.label(RichText::new(format!("{:+.1} dB", eq.preamp)).size(13.0).color(p.on_surface_variant));
        changed |= db_slider(ui, &mut eq.preamp, -24.0, 12.0, 220.0, p.secondary);
    });
    ui.separator();

    // The selected band.
    let n_bands = eq.bands.len();
    if let Some(b) = eq.bands.get_mut(st.band) {
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            ui.label(widgets::title_text(format!("Band {}", st.band + 1), 14.0).color(p.primary));
            ui.add_space(6.0);
            for kind in BandKind::ALL {
                if widgets::filter_chip(ui, kind.name(), b.kind == kind, None).clicked() && b.kind != kind {
                    b.kind = kind;
                    if !kind.has_gain() {
                        b.gain = 0.0;
                    }
                    changed = true;
                }
            }
        });
        row(ui, "Frequency", "", |ui| {
            ui.label(RichText::new(freq_label(b.freq)).size(13.0).color(p.on_surface_variant));
            changed |= log_slider(ui, &mut b.freq, 20.0, 20000.0, 260.0, p.primary);
        });
        if b.kind.has_gain() {
            row(ui, "Gain", "", |ui| {
                ui.label(RichText::new(format!("{:+.1} dB", b.gain)).size(13.0).color(p.on_surface_variant));
                changed |= db_slider(ui, &mut b.gain, -15.0, 15.0, 260.0, p.primary);
            });
        }
        row(ui, "Width (Q)", "Higher is narrower.", |ui| {
            ui.label(RichText::new(format!("{:.2}", b.q)).size(13.0).color(p.on_surface_variant));
            changed |= log_slider(ui, &mut b.q, 0.1, 10.0, 260.0, p.primary);
        });
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if widgets::button_enabled(ui, ButtonKind::Tonal, Some(ICON_ADD), "Add band", n_bands < MAX_BANDS).clicked() {
            let f = eq.bands.get(st.band).map_or(1000.0, |b| (b.freq * 1.6).min(18000.0));
            eq.bands.push(Band { kind: BandKind::Peaking, freq: f, gain: 0.0, q: 1.0 });
            eq.bands.sort_by(|a, b| a.freq.total_cmp(&b.freq));
            st.band = eq.bands.iter().position(|b| b.freq == f).unwrap_or(0);
            changed = true;
        }
        if widgets::button_enabled(ui, ButtonKind::Text, Some(ICON_DELETE), "Remove band", n_bands > 1).clicked() {
            eq.bands.remove(st.band);
            st.band = st.band.saturating_sub(1);
            changed = true;
        }
        if widgets::button(ui, ButtonKind::Text, Some(ICON_RESTART_ALT), "Flatten").clicked() {
            for b in &mut eq.bands {
                b.gain = 0.0;
            }
            eq.preamp = 0.0;
            changed = true;
        }
    });
    if changed {
        let mut new = e.clone();
        new.kind = EffectKind::Equalizer(eq);
        out.push(Action::UpdateEffect(new));
    }
}

/// The curve, with a handle per band. Returns true when a band moved.
fn response_graph(ui: &mut Ui, eq: &mut Equalizer, selected: &mut usize, rate: f32, id: Id) -> bool {
    let p = widgets::palette(ui);
    let width = ui.available_width();
    let (rect, bg) = ui.allocate_exact_size(vec2(width, 250.0), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(16), p.surface_container_high);
    let plot = Rect::from_min_max(rect.min + vec2(44.0, 14.0), rect.max - vec2(16.0, 28.0));
    let peak = eq.bands.iter().map(|b| b.gain.abs()).fold(eq.preamp.abs(), f32::max);
    let range = if peak > 14.0 { 24.0 } else { 15.0 };
    let x_of = |f: f32| plot.left() + plot.width() * freq_t(f);
    let y_of = |db: f32| plot.center().y - (db / range).clamp(-1.0, 1.0) * plot.height() / 2.0;
    let grid = p.outline_variant.gamma_multiply(0.5);
    for db in [-12.0f32, -6.0, 0.0, 6.0, 12.0] {
        if db.abs() > range {
            continue;
        }
        let y = y_of(db);
        let stroke = if db == 0.0 { Stroke::new(1.2, p.outline_variant) } else { Stroke::new(1.0, grid) };
        painter.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], stroke);
        painter.text(pos2(plot.left() - 8.0, y), Align2::RIGHT_CENTER, format!("{db:+.0}"), FontId::proportional(11.0), p.on_surface_variant);
    }
    for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
        let x = x_of(f);
        painter.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, grid));
        painter.text(pos2(x, plot.bottom() + 6.0), Align2::CENTER_TOP, freq_label(f).replace(" kHz", "k").replace(" Hz", ""), FontId::proportional(11.0), p.on_surface_variant);
    }
    // The selected band alone, faintly.
    if let Some(b) = eq.bands.get(*selected) {
        let pts: Vec<_> = (0..=160).map(|i| {
            let f = log_freq(i as f32 / 160.0);
            pos2(x_of(f), y_of(crate::effects::band_response_db(b, f, rate)))
        }).collect();
        painter.add(Shape::line(pts, Stroke::new(1.5, p.tertiary.gamma_multiply(0.7))));
    }
    // The whole curve, filled toward 0 dB.
    let pts: Vec<_> = (0..=240).map(|i| {
        let f = log_freq(i as f32 / 240.0);
        pos2(x_of(f), y_of(eq.response_db(f, rate)))
    }).collect();
    let zero = y_of(0.0);
    let mut mesh = Mesh::default();
    let fill = p.primary.gamma_multiply(0.18);
    for w in pts.windows(2) {
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(w[0], fill);
        mesh.colored_vertex(w[1], fill);
        mesh.colored_vertex(pos2(w[1].x, zero), fill);
        mesh.colored_vertex(pos2(w[0].x, zero), fill);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(Shape::mesh(mesh));
    painter.add(Shape::line(pts, Stroke::new(2.5, p.primary)));

    let mut changed = false;
    // Handles.
    for (k, b) in eq.bands.iter_mut().enumerate() {
        let c = pos2(x_of(b.freq), y_of(if b.kind.has_gain() { b.gain } else { 0.0 }));
        let resp = ui.interact(Rect::from_center_size(c, vec2(26.0, 26.0)), id.with(k), Sense::click_and_drag());
        if resp.clicked() || resp.drag_started() {
            *selected = k;
        }
        if resp.dragged() {
            if let Some(pp) = resp.interact_pointer_pos() {
                let t = ((pp.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
                b.freq = (log_freq(t) * 10.0).round() / 10.0;
                if b.kind.has_gain() {
                    let db = (plot.center().y - pp.y) / (plot.height() / 2.0) * range;
                    b.gain = (db.clamp(-15.0, 15.0) * 10.0).round() / 10.0;
                }
                changed = true;
            }
        }
        if resp.hovered() {
            let scroll = kit::take_wheel(ui);
            if scroll != 0.0 {
                b.q = (b.q * if scroll > 0.0 { 1.12 } else { 1.0 / 1.12 }).clamp(0.1, 10.0);
                changed = true;
            }
        }
        let sel = *selected == k;
        let (fill, fg) = if sel { (p.primary, p.on_primary) } else { (p.secondary_container, p.on_secondary_container) };
        let r = if resp.hovered() || resp.dragged() { 12.0 } else { 10.0 };
        painter.circle_filled(c, r + 2.0, p.surface_container_high);
        painter.circle_filled(c, r, fill);
        painter.text(c, Align2::CENTER_CENTER, (k + 1).to_string(), widgets::title_font(11.0), fg);
        let tip = format!("{} · {} · {:+.1} dB · Q {:.2}\nDrag to move, scroll to change the width", b.kind.name(), freq_label(b.freq), b.gain, b.q);
        resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::Grab);
    }
    // Double-click empty space for a new band there.
    if bg.double_clicked() && eq.bands.len() < MAX_BANDS {
        if let Some(pp) = bg.interact_pointer_pos() {
            let t = ((pp.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
            let db = ((plot.center().y - pp.y) / (plot.height() / 2.0) * range).clamp(-15.0, 15.0);
            let f = (log_freq(t) * 10.0).round() / 10.0;
            eq.bands.push(Band { kind: BandKind::Peaking, freq: f, gain: (db * 10.0).round() / 10.0, q: 1.0 });
            eq.bands.sort_by(|a, b| a.freq.total_cmp(&b.freq));
            *selected = eq.bands.iter().position(|b| b.freq == f).unwrap_or(0);
            changed = true;
        }
    }
    if bg.hovered() {
        bg.on_hover_text("Double-click to add a band");
    }
    changed
}
