//! The mixer: every app and device as a row with its volume, mute,
//! default device, and a menu of where its sound goes.

use crate::app::{status_banner, Action, View};
use crate::graph::{Kind, ObjId};
use crate::ui::kit;
use crate::units::{self, Unit};
use crate::volume;
use egui::{pos2, vec2, CornerRadius, FontId, Frame, Margin, Rect, RichText, Sense, Ui, UiBuilder};
use material_3_ui::common;
use material_3_ui::icons::icons::*;
use material_3_ui::widgets;

pub fn show(ui: &mut Ui, view: &View, out: &mut Vec<Action>) {
    common::wide_page(ui, ICON_TUNE, "Mixer", "Every app and device: how loud, and where its sound goes.", 1100.0, |ui| {
        status_banner(ui, view, out);
        hero(ui, view, out);
        let is_sink = |u: &Unit| view.graph.nodes.get(&u.main).is_some_and(|n| n.is_sink());
        type Pick<'a> = Box<dyn Fn(&Unit) -> bool + 'a>;
        let groups: [(&str, &str, Pick); 4] = [
            ("Playback", "Nothing is playing.", Box::new(|u: &Unit| u.kind == Kind::Player)),
            ("Outputs", "No outputs.", Box::new(|u: &Unit| u.kind == Kind::Output || u.kind == Kind::Effect || (u.kind == Kind::Virtual && is_sink(u)))),
            ("Inputs", "No microphones.", Box::new(|u: &Unit| u.kind == Kind::Input || (u.kind == Kind::Virtual && !is_sink(u)))),
            ("Recording", "Nothing is recording.", Box::new(|u: &Unit| u.kind == Kind::Recorder)),
        ];
        for (title, empty, pick) in groups {
            let rows: Vec<usize> = (0..view.units.len()).filter(|&i| pick(&view.units[i])).collect();
            if rows.is_empty() && (title == "Recording" || title == "Inputs") {
                continue;
            }
            common::card(ui, title, |ui| {
                if rows.is_empty() {
                    common::empty_hint(ui, empty);
                }
                for (n, i) in rows.iter().enumerate() {
                    if n > 0 {
                        ui.separator();
                    }
                    row(ui, view, *i, out);
                }
            });
        }
    });
}

/// The default output and input, big.
fn hero(ui: &mut Ui, view: &View, out: &mut Vec<Action>) {
    let p = widgets::palette(ui);
    let pick = |sink: bool| {
        view.units
            .iter()
            .enumerate()
            .find(|(_, u)| u.is_default && view.graph.nodes.get(&u.main).is_some_and(|n| if sink { n.is_sink() } else { n.is_source() }))
    };
    let tiles: Vec<(&str, (usize, &Unit))> = [("Output", pick(true)), ("Input", pick(false))]
        .into_iter()
        .filter_map(|(label, u)| u.map(|u| (label, u)))
        .collect();
    if tiles.is_empty() {
        return;
    }
    ui.add_space(18.0);
    ui.columns(tiles.len(), |cols| {
        for (col, (label, (_, u))) in cols.iter_mut().zip(&tiles) {
            let Some(v) = view.volume(u.main) else { continue };
            let (bg, fg) = if *label == "Output" { (p.primary_container, p.on_primary_container) } else { (p.tertiary_container, p.on_tertiary_container) };
            Frame::new().fill(bg).corner_radius(CornerRadius::same(24)).inner_margin(Margin::same(18)).show(col, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    widgets::icon(ui, kit::unit_icon(view.graph, u), 24.0, fg);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(format!("Default {}", label.to_lowercase())).size(12.5).color(fg.gamma_multiply(0.8)));
                        let w = ui.available_width();
                        ui.label(widgets::truncated_galley(ui, &u.title, widgets::title_font(17.0), fg, w));
                    });
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if kit::mute_button(ui, v.mute, 36.0).clicked() {
                        out.push(Action::Volume { node: u.main, channels: v.channels.clone(), mute: !v.mute });
                    }
                    let mut m = volume::master(&v);
                    let w = ui.available_width() - 50.0;
                    let color = if v.mute { p.outline } else if *label == "Output" { p.primary } else { p.tertiary };
                    if kit::slider(ui, &mut m, view.cfg.max_volume(), w, color, view.cfg.allow_boost.then_some(1.0)).changed() {
                        out.push(Action::Volume { node: u.main, channels: volume::with_master(&v, m), mute: v.mute });
                    }
                    ui.label(widgets::title_text(volume::percent(m), 15.0).color(fg));
                });
            });
        }
    });
}

/// Where a unit's sound goes, as a chip with a menu of destinations.
fn routes_chip(ui: &mut Ui, view: &View, i: usize, out: &mut Vec<Action>) {
    let u = &view.units[i];
    let sends = view.sends_of(i);
    let label = match sends.as_slice() {
        [] => "Not connected".to_string(),
        [one] => view.units[*one].title.clone(),
        [one, rest @ ..] => format!("{} +{}", view.units[*one].title, rest.len()),
    };
    let chip = widgets::filter_chip(ui, &label, !sends.is_empty(), Some(ICON_ARROW_FORWARD)).on_hover_text("Where the sound goes");
    egui::Popup::menu(&chip).show(|ui| {
        let p = widgets::palette(ui);
        ui.label(RichText::new("Send to").size(12.0).color(p.on_surface_variant));
        let targets: Vec<usize> = (0..view.units.len())
            .filter(|&j| j != i && !view.units[j].inputs.is_empty())
            .filter(|&j| matches!(view.units[j].kind, Kind::Output | Kind::Effect | Kind::Virtual | Kind::Recorder))
            .collect();
        for j in targets {
            let t = &view.units[j];
            let connected = sends.contains(&j);
            if widgets::menu_item(ui, Some(kit::unit_icon(view.graph, t)), &t.title, "", connected) {
                if connected {
                    let links: Vec<ObjId> = view.cords.iter().filter(|c| c.from == i && c.to == j).flat_map(|c| c.links.clone()).collect();
                    out.push(Action::Disconnect(links));
                } else {
                    out.push(Action::Connect(units::plan_connect(view.graph, u, t)));
                }
            }
        }
        if !sends.is_empty() {
            ui.separator();
            if widgets::menu_item(ui, Some(ICON_LINK_OFF), "Disconnect everything", "", false) {
                let links: Vec<ObjId> = view.cords.iter().filter(|c| c.from == i).flat_map(|c| c.links.clone()).collect();
                out.push(Action::Disconnect(links));
            }
        }
    });
}

fn row(ui: &mut Ui, view: &View, i: usize, out: &mut Vec<Action>) {
    let p = widgets::palette(ui);
    let u = &view.units[i];
    let node = view.graph.nodes.get(&u.main);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 58.0), Sense::hover());

    // Trailing controls, right to left.
    let mut trailing = ui.new_child(UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(egui::Align::Center)).id_salt(("mixer-trail", &u.key)));
    trailing.spacing_mut().item_spacing.x = 6.0;
    if !u.outputs.is_empty() && !matches!(u.kind, Kind::Output) {
        routes_chip(&mut trailing, view, i, out);
    } else if u.kind == Kind::Output {
        let sources = view.sources_of(i);
        let text = match sources.len() {
            0 => "Silent".to_string(),
            1 => format!("From {}", view.units[sources[0]].title),
            n => format!("From {n} sources"),
        };
        widgets::chip(&mut trailing, &text, "");
    }
    if node.is_some_and(|n| n.is_sink() || n.is_source()) {
        let tip = if u.is_default { "The default" } else { "Make this the default" };
        if widgets::icon_button_sized(&mut trailing, ICON_STAR, tip, !u.is_default, 34.0, u.is_default).clicked() {
            out.push(Action::SetDefault(u.main));
        }
    }
    let trailing_w = trailing.min_rect().width();

    // Icon, title and subtitle.
    let (bg, fg, strong) = kit::accent(&p, u.kind);
    kit::icon_badge(ui, pos2(rect.left() + 20.0, rect.center().y), 20.0, kit::unit_icon(view.graph, u), bg, fg);
    let text_left = rect.left() + 54.0;
    let text_w = (width * 0.26).clamp(110.0, 240.0);
    let title_r = Rect::from_min_size(pos2(text_left, rect.top() + 6.0), vec2(text_w, rect.height() - 12.0));
    let title_resp = ui.interact(title_r, egui::Id::unique(("mixer-title", &u.key)), Sense::click()).on_hover_text("Show in the patchbay");
    if title_resp.clicked() {
        out.push(Action::Inspect(u.key.clone()));
    }
    let title_color = if title_resp.hovered() { p.primary } else { p.on_surface };
    let t = widgets::truncated_galley(ui, &u.title, widgets::title_font(14.5), title_color, text_w);
    ui.painter().galley(pos2(text_left, rect.center().y - t.size().y - 1.0), t, title_color);
    let sub_color = if u.running && u.kind == Kind::Player { p.primary } else { p.on_surface_variant };
    let s = widgets::truncated_galley(ui, &u.subtitle, FontId::proportional(12.5), sub_color, text_w);
    ui.painter().galley(pos2(text_left, rect.center().y + 1.0), s, sub_color);

    // Volume in the middle.
    let mid = Rect::from_min_max(pos2(text_left + text_w + 12.0, rect.top()), pos2(rect.right() - trailing_w - 16.0, rect.bottom()));
    if mid.width() < 120.0 {
        return;
    }
    let mut mid_ui = ui.new_child(UiBuilder::new().max_rect(mid).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt(("mixer-vol", &u.key)));
    match view.volume(u.main) {
        Some(v) => {
            if kit::mute_button(&mut mid_ui, v.mute, 34.0).clicked() {
                out.push(Action::Volume { node: u.main, channels: v.channels.clone(), mute: !v.mute });
            }
            let mut m = volume::master(&v);
            let w = mid_ui.available_width() - 50.0;
            let color = if v.mute { p.outline } else if matches!(u.kind, Kind::Output | Kind::Recorder) { p.primary } else { strong };
            if kit::slider(&mut mid_ui, &mut m, view.cfg.max_volume(), w, color, view.cfg.allow_boost.then_some(1.0)).changed() {
                out.push(Action::Volume { node: u.main, channels: volume::with_master(&v, m), mute: v.mute });
            }
            mid_ui.label(RichText::new(volume::percent(m)).size(13.0).color(p.on_surface_variant));
        }
        None => {
            mid_ui.label(RichText::new("No volume control").size(12.5).color(p.on_surface_variant));
        }
    }
}
