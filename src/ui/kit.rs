//! Pieces every page uses: per-kind colors, icons for apps and devices,
//! the Material 3 slider, and the mixer row.

use crate::graph::{Graph, Kind, Media};
use crate::units::Unit;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, Ui};
use material_3_ui::icons::{icons::*, MaterialIcon};
use material_3_ui::theme::M3Palette;
use material_3_ui::widgets;

/// Container, content-on-container and strong colors for a kind. Sources
/// are colorful, destinations neutral, so color flows along the cords.
pub fn accent(p: &M3Palette, kind: Kind) -> (Color32, Color32, Color32) {
    match kind {
        Kind::Player => (p.primary_container, p.on_primary_container, p.primary),
        Kind::Input => (p.tertiary_container, p.on_tertiary_container, p.tertiary),
        Kind::Effect | Kind::Virtual => (p.secondary_container, p.on_secondary_container, p.secondary),
        Kind::Output | Kind::Recorder => (p.surface_container_highest, p.on_surface, p.outline),
        Kind::Other => (p.surface_variant, p.on_surface_variant, p.outline),
    }
}

fn has(hay: &str, words: &[&str]) -> bool {
    words.iter().any(|w| hay.contains(w))
}

/// An icon for a card, from what kind of app or device it is.
pub fn unit_icon(g: &Graph, u: &Unit) -> MaterialIcon {
    let Some(n) = g.nodes.get(&u.main) else { return ICON_GRAPHIC_EQ };
    let hay = [&Some(n.name.clone()), &n.app, &n.binary, &n.icon_name, &n.description, &n.nick]
        .iter()
        .filter_map(|s| s.as_deref())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let role = n.role.as_deref().unwrap_or("");
    match u.kind {
        Kind::Player | Kind::Recorder => {
            if has(&hay, &["obs", "record", "audacity", "kooha", "screen-recorder", "ardour", "reaper"]) {
                ICON_RADIO_BUTTON_CHECKED
            } else if has(&hay, &["discord", "vesktop", "webcord", "slack", "teams", "zoom", "telegram", "signal", "element", "mumble", "teamspeak", "webrtc", "skype"]) {
                if u.kind == Kind::Recorder {
                    ICON_HEADSET_MIC
                } else {
                    ICON_FORUM
                }
            } else if has(&hay, &["spotify", "rhythmbox", "amberol", "tidal", "deezer", "elisa", "strawberry", "cider", "lollypop", "cmus", "mpd", "music"]) {
                ICON_LIBRARY_MUSIC
            } else if has(&hay, &["firefox", "chrom", "brave", "vivaldi", "zen", "edge", "opera", "librewolf", "browser", "epiphany", "qutebrowser", "floorp"]) {
                ICON_LANGUAGE
            } else if has(&hay, &["mpv", "vlc", "celluloid", "totem", "kodi", "jellyfin", "plex", "haruna", "showtime", "movie"]) {
                ICON_MOVIE
            } else if has(&hay, &["steam", "game", "wine", "proton", "lutris", "heroic", "retroarch", "minecraft", "gamescope"]) {
                ICON_SPORTS_ESPORTS
            } else {
                match role {
                    "Music" => ICON_MUSIC_NOTE,
                    "Movie" | "Video" => ICON_MOVIE,
                    "Communication" | "Phone" => ICON_FORUM,
                    "Game" => ICON_SPORTS_ESPORTS,
                    _ if u.kind == Kind::Recorder => ICON_KEYBOARD_VOICE,
                    _ => ICON_MUSIC_NOTE,
                }
            }
        }
        Kind::Output => {
            if has(&hay, &["headphone", "headset", "bluez", "earbud", "buds", "airpods"]) {
                ICON_HEADPHONES
            } else if has(&hay, &["hdmi", "displayport", "monitor", "tv"]) {
                ICON_TV
            } else {
                ICON_SPEAKER
            }
        }
        Kind::Input => {
            if has(&hay, &["headset", "bluez"]) {
                ICON_HEADSET_MIC
            } else {
                ICON_MIC
            }
        }
        Kind::Effect => {
            if n.name.starts_with(crate::effects::NODE_PREFIX) {
                ICON_EQUALIZER
            } else {
                ICON_TUNE
            }
        }
        Kind::Virtual => {
            if n.is_source() {
                ICON_INPUT
            } else {
                ICON_SPEAKER_GROUP
            }
        }
        Kind::Other => match u.media {
            Media::Midi => ICON_PIANO,
            Media::Video => ICON_VIDEOCAM,
            _ => ICON_HUB,
        },
    }
}

/// A Material 3 slider: a thick rounded track split by a bar handle, a
/// stop dot at the end, and an optional tick (at 100% when boosting).
/// Scrolling over it nudges the value. Returns a changed response while
/// the value moves.
pub fn slider(ui: &mut Ui, value: &mut f32, max: f32, width: f32, color: Color32, tick: Option<f32>) -> Response {
    let p = widgets::palette(ui);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width.max(40.0), 28.0), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(rect.width() - 8.0, 12.0));
    let to_t = |v: f32| (v / max).clamp(0.0, 1.0);
    if resp.is_pointer_button_down_on() || resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
            let mut v = t * max;
            // A gentle detent at the tick.
            if let Some(tk) = tick {
                if (v - tk).abs() < max * 0.02 {
                    v = tk;
                }
            }
            if (v - *value).abs() > f32::EPSILON {
                *value = v;
                resp.mark_changed();
            }
        }
    }
    // Not on the patchbay, where the wheel pans the canvas.
    if resp.hovered() && ui.ctx().layer_transform_to_global(ui.layer_id()).is_none() {
        let steps = take_wheel(ui);
        if steps != 0.0 {
            *value = (*value + steps.signum() * 0.02 * max.min(1.0)).clamp(0.0, max);
            resp.mark_changed();
        }
    }
    if resp.has_focus() {
        let (l, r) = ui.input(|i| (i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight)));
        if l || r {
            *value = (*value + if r { 0.05 } else { -0.05 }).clamp(0.0, max);
            resp.mark_changed();
        }
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let t = to_t(*value);
        let x = track.left() + track.width() * t;
        let gap = 5.0;
        let active = Rect::from_min_max(track.min, pos2((x - gap).max(track.left()), track.max.y));
        let inactive = Rect::from_min_max(pos2((x + gap).min(track.right()), track.min.y), track.max);
        let (outer, inner) = (CornerRadius::same(6), CornerRadius::same(2));
        if active.width() > 1.0 {
            painter.rect_filled(active, CornerRadius { nw: outer.nw, sw: outer.sw, ne: inner.ne, se: inner.se }, color);
        }
        if inactive.width() > 1.0 {
            painter.rect_filled(inactive, CornerRadius { nw: inner.nw, sw: inner.sw, ne: outer.ne, se: outer.se }, p.secondary_container);
            painter.circle_filled(pos2(track.right() - 6.0, track.center().y), 2.0, color);
        }
        if let Some(tk) = tick {
            let tx = track.left() + track.width() * to_t(tk);
            if (tx - x).abs() > 8.0 {
                let c = if tx < x { p.on_primary } else { color };
                painter.circle_filled(pos2(tx, track.center().y), 2.0, c);
            }
        }
        let handle_h = if resp.dragged() || resp.is_pointer_button_down_on() { 28.0 } else { 24.0 };
        let handle_w = if resp.dragged() { 3.0 } else { 4.0 };
        painter.rect_filled(Rect::from_center_size(pos2(x, track.center().y), vec2(handle_w, handle_h)), CornerRadius::same(2), color);
        if resp.has_focus() {
            painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, p.outline_variant), egui::StrokeKind::Inside);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Mouse-wheel steps this frame (up is positive), taken so the page
/// doesn't scroll too.
pub fn take_wheel(ui: &Ui) -> f32 {
    let steps: f32 = ui.input(|i| {
        i.raw
            .events
            .iter()
            .map(|e| match e {
                egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, .. } => delta.y / 50.0,
                egui::Event::MouseWheel { delta, .. } => delta.y,
                _ => 0.0,
            })
            .sum()
    });
    if steps != 0.0 {
        ui.ctx().input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    }
    steps
}

/// A mute toggle: speaker when on, crossed out in the error color when muted.
pub fn mute_button(ui: &mut Ui, muted: bool, size: f32) -> Response {
    let p = widgets::palette(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    if ui.is_rect_visible(rect) {
        let bg = if muted { p.error_container } else { Color32::TRANSPARENT };
        let fill = widgets::state_layer(bg, p.on_surface, &resp);
        if fill != Color32::TRANSPARENT {
            ui.painter().circle_filled(rect.center(), size / 2.0, fill);
        }
        let (icon, color) = if muted { (ICON_VOLUME_OFF, p.on_error_container) } else { (ICON_VOLUME_UP, p.on_surface_variant) };
        widgets::paint_icon(ui, icon, rect.center(), size * 0.58, color);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if muted { "Unmute" } else { "Mute" })
}

/// An icon in a tonal circle.
pub fn icon_badge(ui: &Ui, center: egui::Pos2, radius: f32, icon: MaterialIcon, bg: Color32, fg: Color32) {
    ui.painter().circle_filled(center, radius, bg);
    widgets::paint_icon(ui, icon, center, radius * 1.1, fg);
}

/// A small tonal label ("Default", "Running").
pub fn pill(ui: &mut Ui, text: &str, bg: Color32, fg: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), widgets::title_font(11.5), fg);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 16.0, 22.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(11), bg);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

/// Painted text, right-aligned at `pos`.
pub fn text_right(ui: &Ui, pos: egui::Pos2, text: &str, size: f32, color: Color32) {
    ui.painter().text(pos, Align2::RIGHT_CENTER, text, FontId::proportional(size), color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units;

    #[test]
    fn demo_cards_get_fitting_icons() {
        let sim = crate::demo::desktop();
        let us = units::build(&sim.g, false);
        let icon = |title: &str| unit_icon(&sim.g, us.iter().find(|u| u.title == title).unwrap()).codepoint;
        assert_eq!(icon("Spotify"), ICON_LIBRARY_MUSIC.codepoint);
        assert_eq!(icon("Firefox"), ICON_LANGUAGE.codepoint);
        assert_eq!(icon("Wireless Headphones"), ICON_HEADPHONES.codepoint);
        assert_eq!(icon("Monitor Speakers"), ICON_TV.codepoint);
        assert_eq!(icon("Headphone EQ"), ICON_EQUALIZER.codepoint);
        assert_eq!(icon("OBS Studio"), ICON_RADIO_BUTTON_CHECKED.codepoint);
    }
}
