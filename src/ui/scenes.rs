//! Scenes: save the current wiring, switch between wirings, and keep one
//! active so its cords come back whenever its apps start.

use crate::app::{status_banner, Action, View};
use crate::scenes::Scene;
use egui::{vec2, CornerRadius, Frame, Margin, RichText, Sense, Ui};
use material_3_ui::common::{self, card, row};
use material_3_ui::icons::icons::*;
use material_3_ui::widgets::{self, ButtonKind};

#[derive(Default)]
pub struct State {
    new_name: String,
    renaming: Option<(String, String)>,
    confirm_delete: Option<String>,
}

pub fn show(ui: &mut Ui, view: &View, st: &mut State, out: &mut Vec<Action>) {
    common::page(
        ui,
        ICON_ROUTE,
        "Scenes",
        "Save how everything is wired and switch setups in one click. The active scene reconnects its apps whenever they start.",
        |ui| {
            status_banner(ui, view, out);
            if let Some(active) = &view.cfg.active_scene {
                common::banner(ui, ICON_ROUTE, &format!("“{active}” is active. Cords you draw or remove are saved to it."), false, |ui| {
                    if widgets::button(ui, ButtonKind::Filled, None, "Deactivate").clicked() {
                        out.push(Action::ActivateScene(None));
                    }
                });
            }
            card(ui, "Save the current wiring", |ui| {
                row(ui, "New scene", "Every cord in the patchbay right now, saved by app and device name so it works after restarts.", |ui| {
                    let ok = !st.new_name.trim().is_empty();
                    if widgets::button_enabled(ui, ButtonKind::Filled, Some(ICON_SAVE), "Save", ok).clicked() {
                        out.push(Action::SaveScene(std::mem::take(&mut st.new_name)));
                    }
                    let r = widgets::field(ui, &mut st.new_name, "Gaming, Recording…", 220.0, egui::Id::unique("new-scene"));
                    if ok && r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        out.push(Action::SaveScene(std::mem::take(&mut st.new_name)));
                    }
                });
            });
            if view.cfg.scenes.is_empty() {
                ui.add_space(16.0);
                common::empty_hint(ui, "No scenes yet. Wire things up in the patchbay, then save it here.");
            }
            for s in &view.cfg.scenes {
                scene_card(ui, view, st, s, out);
            }
        },
    );
    if let Some(name) = st.confirm_delete.clone() {
        let p = widgets::palette(ui);
        let mut close = false;
        common::modal(ui.ctx(), &p, "delete-scene", ICON_DELETE, &format!("Delete “{name}”?"), |ui| {
            ui.label(RichText::new("Its cords stay connected; only the saved scene goes.").color(p.on_surface_variant));
            common::dialog_buttons(ui, |ui| {
                if widgets::button(ui, ButtonKind::Danger, None, "Delete").clicked() {
                    out.push(Action::DeleteScene(name.clone()));
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

fn scene_card(ui: &mut Ui, view: &View, st: &mut State, s: &Scene, out: &mut Vec<Action>) {
    let p = widgets::palette(ui);
    let active = view.cfg.active_scene.as_deref() == Some(s.name.as_str());
    ui.add_space(16.0);
    let fill = if active { p.secondary_container.gamma_multiply(0.55) } else { p.surface_container };
    Frame::new().fill(fill).corner_radius(CornerRadius::same(20)).inner_margin(Margin::symmetric(20, 14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            widgets::icon(ui, ICON_ROUTE, 22.0, if active { p.primary } else { p.on_surface_variant });
            match &mut st.renaming {
                Some((old, new)) if *old == s.name => {
                    let r = widgets::field(ui, new, "Name", 220.0, egui::Id::unique(("rename", &s.name)));
                    r.request_focus();
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        out.push(Action::RenameScene(old.clone(), new.clone()));
                        st.renaming = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        st.renaming = None;
                    }
                }
                _ => {
                    ui.label(widgets::title_text(&s.name, 17.0).color(p.on_surface));
                }
            }
            if active {
                crate::ui::kit::pill(ui, "Active", p.primary, p.on_primary);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, ICON_DELETE, "Delete", true).clicked() {
                    st.confirm_delete = Some(s.name.clone());
                }
                if widgets::icon_button(ui, ICON_EDIT, "Rename", true).clicked() {
                    st.renaming = Some((s.name.clone(), s.name.clone()));
                }
                if widgets::icon_button(ui, ICON_SAVE, "Replace with the current wiring", true).clicked() {
                    out.push(Action::UpdateScene(s.name.clone()));
                }
                if widgets::button(ui, ButtonKind::Tonal, Some(ICON_PLAY_ARROW), "Apply once").clicked() {
                    out.push(Action::ApplyScene(s.name.clone()));
                }
            });
        });
        ui.add_space(4.0);
        row(ui, "Keep connected", "Reconnect these cords whenever their apps and devices appear, and save changes you make to it.", |ui| {
            let mut on = active;
            if widgets::switch(ui, &mut on).clicked() {
                out.push(Action::ActivateScene(on.then(|| s.name.clone())));
            }
        });
        ui.separator();
        row(ui, "Exclusive", "Remove other cords from the apps in this scene, like WirePlumber's automatic link to the default output.", |ui| {
            let mut on = s.exclusive;
            if widgets::switch(ui, &mut on).clicked() {
                out.push(Action::SceneExclusive(s.name.clone(), on));
            }
        });
        ui.add_space(6.0);
        let pairs = s.pairs();
        if pairs.is_empty() {
            ui.label(RichText::new("No cords in this scene.").size(13.0).color(p.on_surface_variant));
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (o, i, n) in pairs {
                let live = view.graph.links.values().any(|l| {
                    view.graph.nodes.get(&l.out_node).is_some_and(|x| x.name == o) && view.graph.nodes.get(&l.in_node).is_some_and(|x| x.name == i)
                });
                let label = format!("{} → {}{}", view.title_for(&o), view.title_for(&i), if n == 1 { " (1 ch)".to_string() } else { String::new() });
                if pair_chip(ui, &label, live) {
                    out.push(Action::RemoveScenePair { scene: s.name.clone(), out_node: o.clone(), in_node: i.clone() });
                }
            }
        });
    });
}

/// "Spotify → Headphones" with a dot (connected now?) and a remove button.
/// Returns true when removed.
fn pair_chip(ui: &mut Ui, text: &str, live: bool) -> bool {
    let p = widgets::palette(ui);
    let galley = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(13.0), p.on_surface);
    let size = vec2(galley.size().x + 26.0 + 30.0, 30.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(8), p.surface_container_high);
    ui.painter().rect_stroke(rect, CornerRadius::same(8), egui::Stroke::new(1.0, p.outline_variant), egui::StrokeKind::Inside);
    let dot = egui::pos2(rect.left() + 13.0, rect.center().y);
    if live {
        ui.painter().circle_filled(dot, 4.0, p.primary);
    } else {
        ui.painter().circle_stroke(dot, 3.5, egui::Stroke::new(1.5, p.outline));
    }
    ui.painter().galley(egui::pos2(rect.left() + 24.0, rect.center().y - galley.size().y / 2.0), galley, p.on_surface);
    let x = egui::Rect::from_center_size(egui::pos2(rect.right() - 15.0, rect.center().y), vec2(24.0, 24.0));
    let resp = ui.interact(x, ui.scope_id().with(("pair", text)), Sense::click());
    if resp.hovered() {
        ui.painter().circle_filled(x.center(), 12.0, p.on_surface.gamma_multiply(0.08));
    }
    widgets::paint_icon(ui, ICON_CLOSE, x.center(), 16.0, p.on_surface_variant);
    let tip = if live { "Connected now. Click × to remove from the scene." } else { "Waiting for the app or device. Click × to remove." };
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}
