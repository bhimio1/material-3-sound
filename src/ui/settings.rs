//! Settings: the shared Appearance card, patchbay and volume options,
//! starting effects and scenes, and About.

use crate::app::{AboutInfo, Action};
use crate::config::Config;
use egui::Ui;
use material_3_ui::common::{self, card, row};
use material_3_ui::icons::icons::*;
use material_3_ui::prefs;
use material_3_ui::theme::ThemeManager;
use material_3_ui::widgets::{self, ButtonKind};

/// Returns true when the appearance changed and needs applying.
pub fn show(ui: &mut Ui, cfg: &mut Config, theme: &ThemeManager, about: &AboutInfo, out: &mut Vec<Action>) -> bool {
    let mut applied = false;
    common::page(ui, ICON_SETTINGS, "Settings", "How Sound looks and behaves.", |ui| {
        applied = prefs::appearance_card(ui, &mut cfg.appearance, theme);
        card(ui, "Patchbay", |ui| {
            row(ui, "Animate cords", "Dots flow along cords that are carrying sound.", |ui| {
                widgets::switch(ui, &mut cfg.animate_cords);
            });
            ui.separator();
            row(ui, "Show channels", "A pin per channel on every card, for routing left and right separately.", |ui| {
                widgets::switch(ui, &mut cfg.show_channels);
            });
            ui.separator();
            row(ui, "MIDI and video", "Show MIDI and video nodes as well as audio.", |ui| {
                widgets::switch(ui, &mut cfg.show_other);
            });
            ui.separator();
            row(ui, "Card layout", "Put every card back in its column.", |ui| {
                if widgets::button(ui, ButtonKind::Outlined, Some(ICON_AUTO_AWESOME_MOSAIC), "Arrange").clicked() {
                    out.push(Action::ResetLayout);
                }
                if !cfg.hidden.is_empty() && widgets::button(ui, ButtonKind::Text, Some(ICON_VISIBILITY), &format!("Show {} hidden", cfg.hidden.len())).clicked() {
                    cfg.hidden.clear();
                }
            });
        });
        card(ui, "Volume", |ui| {
            row(ui, "Allow over 100%", "Let sliders go to 150%. Loud, and may distort.", |ui| {
                widgets::switch(ui, &mut cfg.allow_boost);
            });
        });
        card(ui, "Startup", |ui| {
            row(ui, "Restore effects and scenes", "When Sound opens, start your equalizers and buses and reconnect the active scene.", |ui| {
                widgets::switch(ui, &mut cfg.restore_on_start);
            });
            common::note(
                ui,
                "Effects keep running after you close Sound. To have them, and the active scene, back after you log in without opening the window, run `m3-sound --restore` at login (for Hyprland: exec-once = m3-sound --restore).",
            );
        });
        card(ui, "About", |ui| {
            common::facts(
                ui,
                "about",
                &[
                    ("Sound system", about.backend.clone()),
                    ("Server", about.server.clone()),
                    ("Settings file", about.config.clone()),
                    ("Version", env!("CARGO_PKG_VERSION").to_string()),
                ],
            );
        });
    });
    applied
}
