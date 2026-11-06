//! The side panel for the selected card: volume, balance and per-channel
//! levels, default device, and every connection in and out.

use crate::app::{Action, View};
use crate::graph::{Kind, NodeState, ObjId};
use crate::ui::{kit, patchbay};
use crate::units;
use crate::volume;
use egui::{CornerRadius, Frame, Margin, RichText, Ui};
use material_3_ui::icons::icons::*;
use material_3_ui::widgets::{self, ButtonKind};

fn section(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    let p = widgets::palette(ui);
    ui.add_space(12.0);
    ui.label(widgets::title_text(title, 13.0).color(p.primary));
    ui.add_space(4.0);
    Frame::new().fill(p.surface_container).corner_radius(CornerRadius::same(16)).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

/// Draw the inspector for unit `i`. Returns true when it should close.
pub fn show(ui: &mut Ui, view: &View, i: usize, out: &mut Vec<Action>) -> bool {
    let p = widgets::palette(ui);
    let u = &view.units[i];
    let Some(node) = view.graph.nodes.get(&u.main) else { return true };
    let mut close = false;
    Frame::new().fill(p.surface_container_low).corner_radius(CornerRadius::same(24)).inner_margin(Margin::same(16)).show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (bg, fg, _) = kit::accent(&p, u.kind);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                kit::icon_badge(ui, rect.center(), 24.0, kit::unit_icon(view.graph, u), bg, fg);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    if widgets::icon_button(ui, ICON_CLOSE, "Close", true).clicked() {
                        close = true;
                    }
                    let more = widgets::icon_button(ui, ICON_MORE_VERT, "More", true);
                    egui::Popup::menu(&more).show(|ui| patchbay::unit_menu(ui, view, i, out));
                });
            });
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new(&u.title).size(20.0).color(p.on_surface)).wrap());
            ui.add(egui::Label::new(RichText::new(&u.subtitle).size(13.0).color(p.on_surface_variant)).wrap());
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                kit::pill(ui, u.kind.label(), p.secondary_container, p.on_secondary_container);
                if u.is_default {
                    kit::pill(ui, "Default", p.primary_container, p.on_primary_container);
                }
                let state = match node.state {
                    NodeState::Running => "Playing",
                    NodeState::Idle => "Idle",
                    NodeState::Suspended => "Suspended",
                    NodeState::Creating => "Starting",
                    NodeState::Error => "Error",
                };
                kit::pill(ui, state, p.surface_container_highest, p.on_surface_variant);
            });

            if let Some(v) = view.volume(u.main) {
                section(ui, "Volume", |ui| {
                    ui.horizontal(|ui| {
                        if kit::mute_button(ui, v.mute, 36.0).clicked() {
                            out.push(Action::Volume { node: u.main, channels: v.channels.clone(), mute: !v.mute });
                        }
                        let mut m = volume::master(&v);
                        let w = ui.available_width() - 48.0;
                        let color = if v.mute { p.outline } else { p.primary };
                        if kit::slider(ui, &mut m, view.cfg.max_volume(), w, color, view.cfg.allow_boost.then_some(1.0)).changed() {
                            out.push(Action::Volume { node: u.main, channels: volume::with_master(&v, m), mute: v.mute });
                        }
                        ui.label(RichText::new(volume::percent(m)).size(13.0).color(p.on_surface_variant));
                    });
                    let sided = v.map.iter().any(|c| volume::side(c) != 0);
                    if sided && v.channels.len() >= 2 {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("L").size(12.0).color(p.on_surface_variant));
                            let mut b = volume::balance(&v) + 1.0;
                            let w = ui.available_width() - 64.0;
                            if kit::slider(ui, &mut b, 2.0, w, p.tertiary, Some(1.0)).on_hover_text("Balance").changed() {
                                out.push(Action::Volume { node: u.main, channels: volume::with_balance(&v, b - 1.0), mute: v.mute });
                            }
                            ui.label(RichText::new("R").size(12.0).color(p.on_surface_variant));
                            if widgets::icon_button_sized(ui, ICON_BALANCE, "Center", true, 28.0, false).clicked() {
                                out.push(Action::Volume { node: u.main, channels: volume::with_balance(&v, 0.0), mute: v.mute });
                            }
                        });
                    }
                    if v.channels.len() > 1 {
                        ui.add_space(2.0);
                        egui::CollapsingHeader::new(RichText::new("Channels").size(13.0).color(p.on_surface_variant)).id_salt(("chans", u.main)).show(ui, |ui| {
                            for (c, lin) in v.channels.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    let name = v.map.get(c).cloned().unwrap_or_else(|| format!("{}", c + 1));
                                    ui.add_sized([34.0, 20.0], egui::Label::new(RichText::new(name).size(12.0).color(p.on_surface_variant)));
                                    let mut s = volume::to_slider(*lin);
                                    let w = ui.available_width() - 44.0;
                                    if kit::slider(ui, &mut s, view.cfg.max_volume(), w, p.secondary, None).changed() {
                                        let mut chans = v.channels.clone();
                                        chans[c] = volume::to_linear(s);
                                        out.push(Action::Volume { node: u.main, channels: chans, mute: v.mute });
                                    }
                                    ui.label(RichText::new(volume::percent(s)).size(12.0).color(p.on_surface_variant));
                                });
                            }
                        });
                    }
                });
            }

            if node.is_sink() || node.is_source() {
                section(ui, "Default", |ui| {
                    let label = if node.is_sink() { "Default output" } else { "Default input" };
                    let desc = if node.is_sink() { "New apps play here." } else { "New apps record from here." };
                    material_3_ui::common::row(ui, label, desc, |ui| {
                        let mut on = u.is_default;
                        if widgets::switch(ui, &mut on).clicked() && on {
                            out.push(Action::SetDefault(u.main));
                        }
                    });
                });
            }

            if !u.outputs.is_empty() {
                connections(ui, view, i, true, out);
            }
            if !u.inputs.is_empty() {
                connections(ui, view, i, false, out);
            }

            if let Some(e) = crate::effects::effect_for_node(&view.cfg.effects, &node.name) {
                ui.add_space(14.0);
                if widgets::button(ui, ButtonKind::Tonal, Some(ICON_TUNE), &format!("Edit {}", e.kind_label().to_lowercase())).clicked() {
                    out.push(Action::EditEffect(e.id.clone()));
                }
            }

            ui.add_space(12.0);
            egui::CollapsingHeader::new(RichText::new("Details").size(13.0).color(p.on_surface_variant)).id_salt(("details", u.main)).show(ui, |ui| {
                let ids = u.nodes.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ");
                let mut facts = vec![("Node", node.name.clone()), ("Class", node.media_class.clone()), ("Id", ids)];
                if let Some(app) = &node.binary {
                    facts.push(("Program", app.clone()));
                }
                facts.push(("Ports", format!("{} in, {} out", u.inputs.len(), u.outputs.len())));
                let facts: Vec<(&str, String)> = facts.into_iter().collect();
                material_3_ui::common::facts(ui, &format!("facts-{}", u.main), &facts);
            });
        });
    });
    close
}

fn connections(ui: &mut Ui, view: &View, i: usize, outgoing: bool, out: &mut Vec<Action>) {
    let p = widgets::palette(ui);
    let u = &view.units[i];
    let title = if outgoing { if u.monitor_out { "Monitor goes to" } else { "Sends to" } } else { "Takes from" };
    let peers: Vec<(usize, Vec<ObjId>)> = view
        .cords
        .iter()
        .filter(|c| if outgoing { c.from == i } else { c.to == i })
        .map(|c| (if outgoing { c.to } else { c.from }, c.links.clone()))
        .collect();
    section(ui, title, |ui| {
        if peers.is_empty() {
            ui.label(RichText::new("Nothing yet.").size(13.0).color(p.on_surface_variant));
        }
        for (peer, links) in &peers {
            let pu = &view.units[*peer];
            let subtitle = format!("{} channel{}", links.len(), if links.len() == 1 { "" } else { "s" });
            material_3_ui::common::list_row(ui, kit::unit_icon(view.graph, pu), false, &pu.title, &subtitle, |ui| {
                if widgets::icon_button(ui, ICON_LINK_OFF, "Disconnect", true).clicked() {
                    out.push(Action::Disconnect(links.clone()));
                }
            });
        }
        // Everything that could be connected but isn't.
        let candidates: Vec<usize> = (0..view.units.len())
            .filter(|&j| j != i && !peers.iter().any(|(pi, _)| *pi == j))
            .filter(|&j| if outgoing { !view.units[j].inputs.is_empty() } else { !view.units[j].outputs.is_empty() })
            .filter(|&j| outgoing || view.units[j].kind != Kind::Output || view.units[j].monitor_out)
            .collect();
        if !candidates.is_empty() {
            ui.add_space(4.0);
            let add = widgets::button(ui, ButtonKind::Text, Some(ICON_ADD), "Connect…");
            egui::Popup::menu(&add).show(|ui| {
                for j in candidates {
                    let pu = &view.units[j];
                    if widgets::menu_item(ui, Some(kit::unit_icon(view.graph, pu)), &pu.title, "", false) {
                        let (from, to) = if outgoing { (u, pu) } else { (pu, u) };
                        out.push(Action::Connect(units::plan_connect(view.graph, from, to)));
                    }
                }
            });
        }
    });
}
