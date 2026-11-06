//! The patchbay: a pannable, zoomable canvas of cards (apps, effects,
//! devices) with cords between them. Drag from a jack to a card to
//! connect; click a cord to select it, then press Delete or its unplug
//! button. With channels shown, each port gets its own pin.

use crate::app::{status_banner, Action, View};
use crate::graph::{Kind, ObjId, Port};
use crate::ui::{inspector, kit};
use crate::units::Unit;
use crate::volume;
use egui::epaint::CubicBezierShape;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder};
use material_3_ui::icons::icons::*;
use material_3_ui::widgets::{self, ButtonKind};
use std::collections::HashMap;

pub const CARD_W: f32 = 264.0;
const HEADER_H: f32 = 60.0;
const VOL_H: f32 = 40.0;
const PIN_H: f32 = 22.0;
const COL_X: [f32; 3] = [0.0, 380.0, 760.0];
const GAP: f32 = 20.0;

#[derive(Clone, Debug)]
struct Drag {
    key: String,
    /// Dragging from an output (else from an input).
    output: bool,
    port: Option<ObjId>,
}

pub struct State {
    scene_rect: Rect,
    pub selected: Option<String>,
    selected_cord: Option<(String, String, Option<ObjId>)>,
    pub fit: bool,
    drag: Option<Drag>,
    menu_cord: Option<(String, String, Option<ObjId>)>,
    shown_once: bool,
    /// The canvas size last frame, to keep the zoom when it changes.
    last_canvas: egui::Vec2,
}

impl Default for State {
    fn default() -> Self {
        Self { scene_rect: Rect::ZERO, selected: None, selected_cord: None, fit: true, drag: None, menu_cord: None, shown_once: false, last_canvas: egui::Vec2::ZERO }
    }
}

/// A cord as drawn: its points, which cord (and link, with channels shown)
/// it is, its color, whether sound flows, and its width.
type DrawnCord = (Vec<Pos2>, (String, String, Option<ObjId>), Color32, bool, f32);

fn column(kind: Kind) -> usize {
    match kind {
        Kind::Player | Kind::Input => 0,
        Kind::Effect | Kind::Virtual | Kind::Other => 1,
        Kind::Output | Kind::Recorder => 2,
    }
}

fn card_height(view: &View, u: &Unit) -> f32 {
    let mut h = HEADER_H + 6.0;
    if view.volume(u.main).is_some() {
        h += VOL_H;
    }
    if view.cfg.show_channels {
        let n = u.inputs.len().max(u.outputs.len());
        if n > 0 {
            h += n as f32 * PIN_H + 8.0;
        }
    }
    h
}

/// Where each visible card goes: where the user put it, else stacked in
/// its column (sources, effects, destinations) below the pinned cards.
fn layout(view: &View, visible: &[usize]) -> HashMap<usize, Rect> {
    let mut rects = HashMap::new();
    let mut col_y = [0.0f32; 3];
    for &i in visible {
        let u = &view.units[i];
        if let Some(p) = view.cfg.positions.get(&u.key) {
            let r = Rect::from_min_size(pos2(p[0], p[1]), vec2(CARD_W, card_height(view, u)));
            for (c, x) in COL_X.iter().enumerate() {
                if (r.left() - x).abs() < CARD_W / 2.0 {
                    col_y[c] = col_y[c].max(r.bottom() + GAP);
                }
            }
            rects.insert(i, r);
        }
    }
    for &i in visible {
        if rects.contains_key(&i) {
            continue;
        }
        let u = &view.units[i];
        let c = column(u.kind);
        let h = card_height(view, u);
        rects.insert(i, Rect::from_min_size(pos2(COL_X[c], col_y[c]), vec2(CARD_W, h)));
        col_y[c] += h + GAP;
    }
    rects
}

fn jack_pos(r: Rect, output: bool) -> Pos2 {
    pos2(if output { r.right() } else { r.left() }, r.top() + HEADER_H / 2.0)
}

/// Pin positions in channel mode, by port id.
fn pin_positions(view: &View, u: &Unit, r: Rect) -> Vec<(ObjId, Pos2, bool)> {
    let top = r.bottom() - (u.inputs.len().max(u.outputs.len()) as f32 * PIN_H + 8.0) + 4.0;
    let mut v = Vec::new();
    for (j, p) in u.inputs.iter().enumerate() {
        v.push((*p, pos2(r.left(), top + j as f32 * PIN_H + PIN_H / 2.0), false));
    }
    for (j, p) in u.outputs.iter().enumerate() {
        v.push((*p, pos2(r.right(), top + j as f32 * PIN_H + PIN_H / 2.0), true));
    }
    let _ = view;
    v
}

fn bezier(a: Pos2, b: Pos2) -> [Pos2; 4] {
    let dx = ((b.x - a.x).abs() * 0.5).max(70.0) + if b.x < a.x { 60.0 } else { 0.0 };
    [a, pos2(a.x + dx, a.y), pos2(b.x - dx, b.y), b]
}

fn sample(points: [Pos2; 4], n: usize) -> Vec<Pos2> {
    let s = CubicBezierShape::from_points_stroke(points, false, Color32::TRANSPARENT, Stroke::NONE);
    (0..=n).map(|i| s.sample(i as f32 / n as f32)).collect()
}

fn dist_to_polyline(p: Pos2, pts: &[Pos2]) -> f32 {
    pts.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
            (a + ab * t - p).length()
        })
        .fold(f32::INFINITY, f32::min)
}

/// Port pairs for a drop: from a card or pin onto a card or pin.
fn plan_drop(view: &View, drag: &Drag, target_unit: usize, target_port: Option<ObjId>) -> Vec<(ObjId, ObjId)> {
    let g = view.graph;
    let Some((src_i, src)) = view.unit(&drag.key) else { return Vec::new() };
    if src_i == target_unit {
        return Vec::new();
    }
    let dst = &view.units[target_unit];
    let (from_unit, to_unit) = if drag.output { (src, dst) } else { (dst, src) };
    let (from_port, to_port) = if drag.output { (drag.port, target_port) } else { (target_port, drag.port) };
    let ports = |ids: &[ObjId]| -> Vec<&Port> { ids.iter().filter_map(|id| g.ports.get(id)).collect() };
    let outs: Vec<&Port> = match from_port {
        Some(p) => g.ports.get(&p).into_iter().collect(),
        None => ports(&from_unit.outputs),
    };
    let ins: Vec<&Port> = match to_port {
        Some(p) => g.ports.get(&p).into_iter().collect(),
        None => ports(&to_unit.inputs),
    };
    // One pin onto a card (or a card onto one pin): match its channel if there is one.
    let pairs = if outs.len() == 1 && ins.len() > 1 {
        match ins.iter().find(|i| !outs[0].channel.is_empty() && i.channel == outs[0].channel) {
            Some(i) if from_port.is_some() => vec![(outs[0].id, i.id)],
            _ => crate::graph::plan_links(&outs, &ins),
        }
    } else if ins.len() == 1 && outs.len() > 1 && to_port.is_some() {
        match outs.iter().find(|o| !o.channel.is_empty() && o.channel == ins[0].channel) {
            Some(o) => vec![(o.id, ins[0].id)],
            None => crate::graph::plan_links(&outs, &ins),
        }
    } else {
        crate::graph::plan_links(&outs, &ins)
    };
    pairs.into_iter().filter(|(o, i)| g.link_between(*o, *i).is_none()).collect()
}

pub fn unit_menu(ui: &mut Ui, view: &View, i: usize, out: &mut Vec<Action>) {
    let u = &view.units[i];
    let Some(n) = view.graph.nodes.get(&u.main) else { return };
    if let Some(v) = view.volume(u.main) {
        let (icon, label) = if v.mute { (ICON_VOLUME_UP, "Unmute") } else { (ICON_VOLUME_OFF, "Mute") };
        if widgets::menu_item(ui, Some(icon), label, "", false) {
            out.push(Action::Volume { node: u.main, channels: v.channels.clone(), mute: !v.mute });
        }
    }
    if (n.is_sink() || n.is_source()) && !u.is_default {
        let label = if n.is_sink() { "Use as default output" } else { "Use as default input" };
        if widgets::menu_item(ui, Some(ICON_STAR), label, "", false) {
            out.push(Action::SetDefault(u.main));
        }
    }
    let links: Vec<ObjId> = view.cords.iter().filter(|c| c.from == i || c.to == i).flat_map(|c| c.links.clone()).collect();
    if !links.is_empty() && widgets::menu_item(ui, Some(ICON_LINK_OFF), "Disconnect everything", "", false) {
        out.push(Action::Disconnect(links));
    }
    if let Some(e) = crate::effects::effect_for_node(&view.cfg.effects, &n.name) {
        if widgets::menu_item(ui, Some(ICON_TUNE), &format!("Edit {}", e.kind_label().to_lowercase()), "", false) {
            out.push(Action::EditEffect(e.id.clone()));
        }
    }
    if widgets::menu_item(ui, Some(ICON_VISIBILITY_OFF), "Hide card", "", false) {
        out.push(Action::Hide(u.key.clone()));
    }
}

fn canvas_menu(ui: &mut Ui, view: &View, st: &mut State, out: &mut Vec<Action>) {
    if widgets::menu_item(ui, Some(ICON_EQUALIZER), "New equalizer", "", false) {
        out.push(Action::AddEqualizer);
    }
    if widgets::menu_item(ui, Some(ICON_SPEAKER_GROUP), "New mix bus", "", false) {
        out.push(Action::AddBus);
    }
    ui.separator();
    if widgets::menu_item(ui, Some(ICON_FIT_SCREEN), "Fit to window", "Ctrl+0", false) {
        st.fit = true;
    }
    if widgets::menu_item(ui, Some(ICON_AUTO_AWESOME_MOSAIC), "Arrange cards", "", false) {
        out.push(Action::ResetLayout);
    }
    if !view.cfg.hidden.is_empty() && widgets::menu_item(ui, Some(ICON_VISIBILITY), "Show hidden cards", "", false) {
        for k in &view.cfg.hidden {
            out.push(Action::Unhide(k.clone()));
        }
    }
}

pub fn show(ui: &mut Ui, view: &View, st: &mut State, out: &mut Vec<Action>) {
    let p = widgets::palette(ui);
    let visible: Vec<usize> = (0..view.units.len()).filter(|i| !view.cfg.hidden.contains(&view.units[*i].key)).collect();
    if let Some(key) = &st.selected {
        if view.unit(key).is_none() {
            st.selected = None;
        }
    }
    // The inspector for the selected card.
    if let Some((i, _)) = st.selected.as_deref().and_then(|k| view.unit(k)) {
        egui::Panel::right("inspector")
            .exact_size(330.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().inner_margin(egui::Margin { left: 0, right: 12, top: 12, bottom: 12 }))
            .show(ui, |ui| {
                if inspector::show(ui, view, i, out) {
                    st.selected = None;
                }
            });
    }
    status_banner(ui, view, out);

    let rects = layout(view, &visible);
    let ctx = ui.ctx().clone();
    if keys_pressed(&ctx, egui::Key::Num0) {
        st.fit = true;
    }
    if st.fit || !st.shown_once {
        if let Some(bounds) = rects.values().copied().reduce(|a, b| a.union(b)) {
            st.scene_rect = Rect::from_min_max(bounds.min - vec2(70.0, 110.0), bounds.max + vec2(70.0, 50.0));
            st.fit = false;
            st.shown_once = true;
        }
    }
    let canvas_rect = ui.available_rect_before_wrap();
    // The inspector opening (or the window resizing) keeps the zoom and
    // the top-left corner instead of refitting the view.
    let last = st.last_canvas;
    if last.x > 1.0 && last.y > 1.0 && (canvas_rect.size() - last).length() > 0.5 && st.scene_rect.width() > 1.0 {
        let scale = st.scene_rect.width() / last.x;
        st.scene_rect = Rect::from_min_size(st.scene_rect.min, canvas_rect.size() * scale);
    }
    st.last_canvas = canvas_rect.size();
    if visible.is_empty() {
        let text = match view.status {
            crate::backend::Status::Connecting => "Connecting to PipeWire…",
            crate::backend::Status::Failed(_) => "Nothing to show until PipeWire is reachable.",
            crate::backend::Status::Connected if view.units.is_empty() => "No sound devices or apps yet. Play something!",
            crate::backend::Status::Connected => "Every card is hidden. Right-click to show them again.",
        };
        ui.painter().text(canvas_rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(15.0), p.on_surface_variant);
    }

    let mut pins: HashMap<ObjId, (Pos2, usize)> = HashMap::new();
    if view.cfg.show_channels {
        for (&i, &r) in &rects {
            for (port, pos, _) in pin_positions(view, &view.units[i], r) {
                pins.insert(port, (pos, i));
            }
        }
    }
    let animate = view.cfg.animate_cords;
    let time = ui.input(|i| i.time) as f32;
    let mut hovered_cord: Option<(String, String, Option<ObjId>)> = None;
    let mut any_active = false;

    let scene = egui::Scene::new().zoom_range(0.25..=1.0).max_inner_size(vec2(1.0e5, 1.0e5));
    let mut scene_rect = st.scene_rect;
    let mut visible_now = None;
    let resp = scene.show(ui, &mut scene_rect, |ui| {
        let from_global = ui.ctx().layer_transform_from_global(ui.layer_id()).unwrap_or_default();
        visible_now = Some(from_global * canvas_rect);
        let pointer = ui.ctx().pointer_latest_pos().map(|pp| from_global * pp);
        let zoom = 1.0 / from_global.scaling.max(1e-3);
        let painter = ui.painter().clone();
        let clip = ui.clip_rect();

        // A faint dot grid.
        let mut step = 28.0;
        while step * zoom < 14.0 {
            step *= 2.0;
        }
        let dot = p.outline_variant.gamma_multiply(0.45);
        let (x0, y0) = ((clip.left() / step).floor() as i64, (clip.top() / step).floor() as i64);
        let (x1, y1) = ((clip.right() / step).ceil() as i64, (clip.bottom() / step).ceil() as i64);
        if (x1 - x0) * (y1 - y0) < 20_000 {
            for x in x0..=x1 {
                for y in y0..=y1 {
                    painter.circle_filled(pos2(x as f32 * step, y as f32 * step), 1.1 / zoom.max(0.5), dot);
                }
            }
        }

        let over_card = pointer.is_some_and(|pp| rects.values().any(|r| r.expand(6.0).contains(pp)));
        let selected_unit = st.selected.as_deref().and_then(|k| view.unit(k)).map(|(i, _)| i);

        // Cords, under the cards.
        let mut drawn: Vec<DrawnCord> = Vec::new();
        for c in view.cords {
            let (Some(&ra), Some(&rb)) = (rects.get(&c.from), rects.get(&c.to)) else { continue };
            let (ua, ub) = (&view.units[c.from], &view.units[c.to]);
            let color = kit::accent(&p, ua.kind).2;
            let mut segments: Vec<([Pos2; 4], Option<ObjId>)> = Vec::new();
            if view.cfg.show_channels {
                for l in &c.links {
                    let Some(link) = view.graph.links.get(l) else { continue };
                    if let (Some((a, _)), Some((b, _))) = (pins.get(&link.out_port), pins.get(&link.in_port)) {
                        segments.push((bezier(*a, *b), Some(*l)));
                    }
                }
            } else {
                segments.push((bezier(jack_pos(ra, true), jack_pos(rb, false)), None));
            }
            for (pts, link) in segments {
                let poly = sample(pts, 24);
                let id = (ua.key.clone(), ub.key.clone(), link);
                let width = if view.cfg.show_channels { 2.2 } else { 3.2 } + if c.links.len() > 2 && !view.cfg.show_channels { 0.8 } else { 0.0 };
                let mut alpha = if c.active { 1.0 } else { 0.55 };
                if selected_unit.is_some_and(|s| s != c.from && s != c.to) {
                    alpha *= 0.3;
                }
                drawn.push((poly, id, color.gamma_multiply(alpha), c.active, width));
                let _ = pts;
            }
        }
        if !over_card && st.drag.is_none() {
            if let Some(pp) = pointer {
                hovered_cord = drawn
                    .iter()
                    .map(|(poly, id, ..)| (dist_to_polyline(pp, poly), id))
                    .filter(|(d, _)| *d < 10.0 / zoom.max(0.05))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, id)| id.clone());
            }
        }
        for (poly, id, color, active, width) in &drawn {
            let hot = hovered_cord.as_ref() == Some(id) || st.selected_cord.as_ref() == Some(id);
            let w = if hot { width + 2.0 } else { *width };
            if hot {
                painter.add(Shape::line(poly.clone(), Stroke::new(w + 6.0, color.gamma_multiply(0.18))));
            }
            painter.add(Shape::line(poly.clone(), Stroke::new(w, *color)));
            if *active && animate {
                any_active = true;
                let len: f32 = poly.windows(2).map(|s| (s[1] - s[0]).length()).sum();
                let n = (len / 46.0).ceil().max(1.0) as usize;
                let dot_color = material_3_ui::theme::palette::blend(*color, Color32::WHITE, if p.is_dark() { 0.65 } else { 0.8 });
                for k in 0..n {
                    let t = ((time * 46.0 / len.max(1.0)) + k as f32 / n as f32).fract();
                    let idx = t * (poly.len() - 1) as f32;
                    let (a, b) = (poly[idx.floor() as usize], poly[(idx.ceil() as usize).min(poly.len() - 1)]);
                    painter.circle_filled(a + (b - a) * idx.fract(), w * 0.55, dot_color);
                }
            }
        }

        // Cards.
        let mut order: Vec<usize> = visible.clone();
        // The selected card draws last, on top.
        order.sort_by_key(|i| Some(*i) == selected_unit);
        let drop_target = st.drag.as_ref().and(pointer).and_then(|pp| order.iter().rev().find(|i| rects[i].expand(8.0).contains(pp)).copied());
        let pin_target = st.drag.as_ref().and(pointer).and_then(|pp| {
            pins.iter().filter(|(_, (pos, _))| (*pos - pp).length() < 12.0).map(|(id, (_, u))| (*id, *u)).next()
        });
        let mut start_move = false;
        for &i in &order {
            let r = rects[&i];
            let u = &view.units[i];
            let highlight = st.drag.as_ref().is_some_and(|d| {
                let ok = if d.output { !u.inputs.is_empty() } else { !u.outputs.is_empty() };
                ok && d.key != u.key && (drop_target == Some(i) || pin_target.is_some_and(|(_, pu)| pu == i))
            });
            if card(ui, view, i, r, st, highlight, out) {
                start_move = true;
            }
        }
        if start_move {
            // Pin every card where it is, so nothing reflows under the drag.
            for &i in &visible {
                let u = &view.units[i];
                if !view.cfg.positions.contains_key(&u.key) {
                    out.push(Action::MoveCard(u.key.clone(), [rects[&i].left(), rects[&i].top()]));
                }
            }
        }

        // Jacks and pins, over the card edges.
        for &i in &order {
            let r = rects[&i];
            let u = &view.units[i];
            let (_, _, strong) = kit::accent(&p, u.kind);
            for output in [false, true] {
                let ports = if output { &u.outputs } else { &u.inputs };
                if ports.is_empty() {
                    continue;
                }
                let connected = view.cords.iter().any(|c| if output { c.from == i } else { c.to == i });
                let center = jack_pos(r, output);
                let monitor = output && u.monitor_out;
                if let Some(d) = jack(ui, view, st, Id::unique(("jack", &u.key, output)), center, strong, connected, monitor, &u.key, output, None, out) {
                    st.drag = d;
                }
            }
            if view.cfg.show_channels {
                for (port, pos, output) in pin_positions(view, u, r) {
                    let connected = view.graph.links.values().any(|l| l.out_port == port || l.in_port == port);
                    if let Some(d) = jack(ui, view, st, Id::unique(("pin", port)), pos, strong, connected, false, &u.key, output, Some(port), out) {
                        st.drag = d;
                    }
                    if let Some(pp) = view.graph.ports.get(&port) {
                        let (align, x) = if output { (Align2::RIGHT_CENTER, pos.x - 14.0) } else { (Align2::LEFT_CENTER, pos.x + 14.0) };
                        let label = if pp.monitor { format!("{} (monitor)", pp.label()) } else { pp.label().to_string() };
                        painter.text(pos2(x, pos.y), align, label, FontId::proportional(11.5), p.on_surface_variant);
                    }
                }
            }
        }

        // The cord being drawn.
        if let (Some(d), Some(pp)) = (&st.drag, pointer) {
            if let Some((i, u)) = view.unit(&d.key) {
                let start = match d.port.and_then(|port| pins.get(&port)) {
                    Some((pos, _)) => *pos,
                    None => jack_pos(rects.get(&i).copied().unwrap_or(Rect::NOTHING), d.output),
                };
                let snap = pin_target.map(|(port, _)| pins[&port].0).unwrap_or(pp);
                let pts = if d.output { bezier(start, snap) } else { bezier(snap, start) };
                let color = kit::accent(&p, u.kind).2;
                painter.add(Shape::line(sample(pts, 24), Stroke::new(3.0, color)));
                painter.circle_filled(snap, 5.0, color);
            }
        }

        // Unplug button on the selected cord.
        if let Some(sel) = st.selected_cord.clone() {
            if let Some((poly, ..)) = drawn.iter().find(|(_, id, ..)| *id == sel) {
                let mid = poly[poly.len() / 2];
                let r = Rect::from_center_size(mid, vec2(28.0, 28.0));
                let b = ui.interact(r, Id::unique("unplug"), Sense::click());
                painter.circle_filled(mid, 15.0, widgets::state_layer(p.error_container, p.on_error_container, &b));
                widgets::paint_icon(ui, ICON_LINK_OFF, mid, 18.0, p.on_error_container);
                if b.on_hover_text("Disconnect (Delete)").clicked() {
                    disconnect_selected(view, st, out);
                }
            } else {
                st.selected_cord = None;
            }
        }
    });
    // Remember exactly what's on screen (the scene letterboxes what it's given).
    st.scene_rect = match visible_now {
        Some(v) if !resp.response.changed() && v.is_finite() && v.width() > 1.0 => v,
        _ => scene_rect,
    };

    // Background clicks select cords or clear the selection.
    let bg = resp.response;
    if bg.clicked() {
        st.selected_cord = hovered_cord.clone();
        if hovered_cord.is_none() {
            st.selected = None;
        }
    }
    if bg.secondary_clicked() {
        st.menu_cord = hovered_cord.clone();
    }
    bg.context_menu(|ui| match st.menu_cord.clone() {
        Some(c) => {
            if widgets::menu_item(ui, Some(ICON_LINK_OFF), "Disconnect", "Delete", false) {
                st.selected_cord = Some(c);
                disconnect_selected(view, st, out);
            }
        }
        None => canvas_menu(ui, view, st, out),
    });
    if st.selected_cord.is_some() && !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
        disconnect_selected(view, st, out);
    }
    if !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        if st.drag.is_some() || st.selected_cord.is_some() {
            st.drag = None;
            st.selected_cord = None;
        } else {
            st.selected = None;
        }
    }
    if hovered_cord.is_some() && st.drag.is_none() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if any_active && animate {
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
    toolbar(ui, view, st, canvas_rect, out);
}

fn keys_pressed(ctx: &egui::Context, key: egui::Key) -> bool {
    material_3_ui::keys::take_key(ctx, egui::Modifiers::COMMAND, key)
}

fn disconnect_selected(view: &View, st: &mut State, out: &mut Vec<Action>) {
    let Some((a, b, link)) = st.selected_cord.take() else { return };
    let ids: Vec<ObjId> = match link {
        Some(l) => vec![l],
        None => match (view.unit(&a), view.unit(&b)) {
            (Some((ia, _)), Some((ib, _))) => view.cords.iter().filter(|c| c.from == ia && c.to == ib).flat_map(|c| c.links.clone()).collect(),
            _ => Vec::new(),
        },
    };
    out.push(Action::Disconnect(ids));
}

/// A jack or pin: drag from it to connect. Returns a new drag state when
/// one starts or ends.
#[allow(clippy::too_many_arguments)]
fn jack(
    ui: &mut Ui,
    view: &View,
    st: &State,
    id: Id,
    center: Pos2,
    color: Color32,
    connected: bool,
    monitor: bool,
    key: &str,
    output: bool,
    port: Option<ObjId>,
    out: &mut Vec<Action>,
) -> Option<Option<Drag>> {
    let p = widgets::palette(ui);
    let size = if port.is_some() { 18.0 } else { 26.0 };
    let resp = ui.interact(Rect::from_center_size(center, vec2(size, size)), id, Sense::click_and_drag());
    let hot = resp.hovered() || resp.dragged();
    let radius = if port.is_some() { 5.0 } else { 7.5 } + if hot { 2.0 } else { 0.0 };
    let painter = ui.painter();
    painter.circle_filled(center, radius + 2.5, p.surface_container_high);
    if connected && !monitor {
        painter.circle_filled(center, radius, color);
    } else {
        painter.circle_filled(center, radius, p.surface_container_highest);
        painter.circle_stroke(center, radius - 1.0, Stroke::new(2.0, if connected { color } else { p.outline }));
        if monitor {
            painter.circle_filled(center, radius * 0.35, if connected { color } else { p.outline });
        }
    }
    let tip = match (output, monitor) {
        (true, true) => "Monitor: what this device plays. Drag to a card to send it on.",
        (true, false) => "Drag to a card to send sound to it",
        (false, _) => "Drag to a card to take sound from it",
    };
    let resp = resp.on_hover_cursor(egui::CursorIcon::Grab).on_hover_text(tip);
    if resp.drag_started() {
        return Some(Some(Drag { key: key.to_string(), output, port }));
    }
    if resp.drag_stopped() {
        if let Some(d) = st.drag.clone() {
            let from_global = ui.ctx().layer_transform_from_global(ui.layer_id()).unwrap_or_default();
            if let Some(pp) = ui.ctx().pointer_latest_pos().map(|pp| from_global * pp) {
                drop(view, &d, pp, out);
            }
        }
        return Some(None);
    }
    None
}

/// Finish a drag at `pp`: connect to the pin or card under it.
fn drop(view: &View, d: &Drag, pp: Pos2, out: &mut Vec<Action>) {
    let visible: Vec<usize> = (0..view.units.len()).filter(|i| !view.cfg.hidden.contains(&view.units[*i].key)).collect();
    let rects = layout(view, &visible);
    let mut target: Option<(usize, Option<ObjId>)> = None;
    if view.cfg.show_channels {
        for (&i, &r) in &rects {
            for (port, pos, output) in pin_positions(view, &view.units[i], r) {
                if output != d.output && (pos - pp).length() < 12.0 {
                    target = Some((i, Some(port)));
                }
            }
        }
    }
    if target.is_none() {
        target = rects.iter().find(|(_, r)| r.expand(8.0).contains(pp)).map(|(i, _)| (*i, None));
    }
    let Some((ti, tport)) = target else { return };
    let tu = &view.units[ti];
    let fits = if d.output { !tu.inputs.is_empty() } else { !tu.outputs.is_empty() };
    if !fits || tu.key == d.key {
        if tu.key != d.key {
            let what = if d.output { "doesn't take sound in" } else { "doesn't send sound out" };
            out.push(Action::Toast(format!("{} {what}.", tu.title)));
        }
        return;
    }
    let pairs = plan_drop(view, d, ti, tport);
    if pairs.is_empty() {
        out.push(Action::Toast("Already connected.".into()));
    } else {
        out.push(Action::Connect(pairs));
    }
}

/// One card. Returns true when the user starts moving it.
fn card(ui: &mut Ui, view: &View, i: usize, r: Rect, st: &mut State, highlight: bool, out: &mut Vec<Action>) -> bool {
    let p = widgets::palette(ui);
    let u = &view.units[i];
    let selected = st.selected.as_deref() == Some(u.key.as_str());
    let head = Rect::from_min_size(r.min, vec2(r.width(), HEADER_H));
    let resp = ui.interact(head, Id::unique(("card", &u.key)), Sense::click_and_drag());
    let mut started = false;
    if resp.drag_started() {
        started = true;
        st.selected = Some(u.key.clone());
    }
    if resp.dragged() {
        let pos = r.min + resp.drag_delta();
        out.push(Action::MoveCard(u.key.clone(), [pos.x.round(), pos.y.round()]));
    }
    if resp.clicked() {
        st.selected = if selected { None } else { Some(u.key.clone()) };
        st.selected_cord = None;
    }
    if resp.double_clicked() {
        if let Some(e) = view.graph.nodes.get(&u.main).and_then(|n| crate::effects::effect_for_node(&view.cfg.effects, &n.name)) {
            out.push(Action::EditEffect(e.id.clone()));
        }
    }
    resp.context_menu(|ui| unit_menu(ui, view, i, out));
    let dragging = resp.dragged();
    let resp = resp.on_hover_cursor(if dragging { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });

    let painter = ui.painter();
    let radius = CornerRadius::same(20);
    painter.add(egui::Shadow { offset: [0, 2], blur: 10, spread: 0, color: Color32::from_black_alpha(if p.is_dark() { 70 } else { 22 }) }.as_shape(r, radius));
    painter.rect_filled(r, radius, p.surface_container_high);
    if resp.hovered() {
        painter.rect_filled(head, CornerRadius { nw: 20, ne: 20, sw: 0, se: 0 }, p.on_surface.gamma_multiply(0.04));
    }
    if selected || highlight {
        painter.rect_stroke(r, radius, Stroke::new(2.0, p.primary), StrokeKind::Outside);
    }
    let (bg, fg, _) = kit::accent(&p, u.kind);
    kit::icon_badge(ui, pos2(r.left() + 32.0, r.top() + HEADER_H / 2.0), 18.0, kit::unit_icon(view.graph, u), bg, fg);
    let text_w = r.width() - 60.0 - 46.0 - if u.is_default { 20.0 } else { 0.0 };
    let title = widgets::truncated_galley(ui, &u.title, widgets::title_font(14.5), p.on_surface, text_w);
    let title_w = title.size().x;
    ui.painter().galley(pos2(r.left() + 60.0, r.top() + 13.0), title, p.on_surface);
    if u.is_default {
        widgets::paint_icon(ui, ICON_STAR, pos2(r.left() + 60.0 + title_w + 11.0, r.top() + 22.0), 15.0, p.primary);
    }
    let sub_color = if u.running && u.kind == Kind::Player { p.primary } else { p.on_surface_variant };
    let sub = widgets::truncated_galley(ui, &u.subtitle, FontId::proportional(12.5), sub_color, r.width() - 60.0 - 46.0);
    ui.painter().galley(pos2(r.left() + 60.0, r.top() + 33.0), sub, sub_color);

    // The ⋮ menu.
    let more_rect = Rect::from_center_size(pos2(r.right() - 26.0, r.top() + HEADER_H / 2.0), vec2(32.0, 32.0));
    let mut more_ui = ui.new_child(UiBuilder::new().max_rect(more_rect).id_salt(("more", &u.key)));
    let more = widgets::icon_button_sized(&mut more_ui, ICON_MORE_VERT, "", true, 32.0, false);
    egui::Popup::menu(&more).show(|ui| unit_menu(ui, view, i, out));

    // Volume.
    if let Some(v) = view.volume(u.main) {
        let row = Rect::from_min_size(pos2(r.left() + 10.0, r.top() + HEADER_H), vec2(r.width() - 20.0, VOL_H - 4.0));
        let mut row_ui = ui.new_child(UiBuilder::new().max_rect(row).id_salt(("vol", &u.key)).layout(egui::Layout::left_to_right(egui::Align::Center)));
        row_ui.spacing_mut().item_spacing.x = 6.0;
        if kit::mute_button(&mut row_ui, v.mute, 32.0).clicked() {
            out.push(Action::Volume { node: u.main, channels: v.channels.clone(), mute: !v.mute });
        }
        let mut m = volume::master(&v);
        let w = row_ui.available_width() - 46.0;
        let (_, _, strong) = kit::accent(&p, if u.kind == Kind::Output || u.kind == Kind::Recorder { Kind::Player } else { u.kind });
        let color = if v.mute { p.outline } else { strong };
        if kit::slider(&mut row_ui, &mut m, view.cfg.max_volume(), w, color, view.cfg.allow_boost.then_some(1.0)).changed() {
            out.push(Action::Volume { node: u.main, channels: volume::with_master(&v, m), mute: v.mute });
        }
        let pct = volume::percent(m);
        kit::text_right(&row_ui, pos2(row.right() - 2.0, row.center().y), &pct, 12.5, p.on_surface_variant);
    }
    if view.cfg.show_channels && (!u.inputs.is_empty() || !u.outputs.is_empty()) {
        let n = u.inputs.len().max(u.outputs.len());
        let top = r.bottom() - (n as f32 * PIN_H + 8.0);
        ui.painter().line_segment([pos2(r.left() + 16.0, top), pos2(r.right() - 16.0, top)], Stroke::new(1.0, p.outline_variant.gamma_multiply(0.6)));
    }
    started
}

/// The floating bar over the canvas.
fn toolbar(ui: &mut Ui, view: &View, st: &mut State, canvas: Rect, out: &mut Vec<Action>) {
    let p = widgets::palette(ui);
    egui::Area::new(Id::unique("patchbay-toolbar"))
        .order(egui::Order::Middle)
        .fixed_pos(canvas.left_top() + vec2(16.0, 16.0))
        .constrain_to(canvas)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(p.surface_container_high)
                .corner_radius(CornerRadius::same(24))
                .inner_margin(egui::Margin::symmetric(8, 6))
                .shadow(egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(40) })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let add = widgets::button(ui, ButtonKind::Tonal, Some(ICON_ADD), "Add");
                        egui::Popup::menu(&add).show(|ui| {
                            if widgets::menu_item(ui, Some(ICON_EQUALIZER), "Equalizer", "", false) {
                                out.push(Action::AddEqualizer);
                            }
                            if widgets::menu_item(ui, Some(ICON_SPEAKER_GROUP), "Mix bus", "", false) {
                                out.push(Action::AddBus);
                            }
                        });
                        ui.add_space(4.0);
                        if widgets::filter_chip(ui, "Channels", view.cfg.show_channels, Some(ICON_LINEAR_SCALE))
                            .on_hover_text("One pin per channel, for left/right routing")
                            .clicked()
                        {
                            out.push(Action::ToggleChannels);
                        }
                        if widgets::icon_button(ui, ICON_FIT_SCREEN, "Fit to window (Ctrl+0)", true).clicked() {
                            st.fit = true;
                        }
                        if widgets::icon_button(ui, ICON_AUTO_AWESOME_MOSAIC, "Arrange cards", true).clicked() {
                            out.push(Action::ResetLayout);
                        }
                        if !view.cfg.hidden.is_empty() {
                            let h = widgets::chip(ui, &format!("{} hidden", view.cfg.hidden.len()), "Show hidden cards");
                            egui::Popup::menu(&h).show(|ui| {
                                for k in &view.cfg.hidden {
                                    let label = k.split_once(':').map(|(_, rest)| rest).unwrap_or(k);
                                    if widgets::menu_item(ui, Some(ICON_VISIBILITY), label, "", false) {
                                        out.push(Action::Unhide(k.clone()));
                                    }
                                }
                            });
                        }
                        ui.add_space(6.0);
                        let label = match &view.cfg.active_scene {
                            Some(s) => format!("Scene: {s}"),
                            None => "No scene".into(),
                        };
                        let chip = widgets::filter_chip(ui, &label, view.cfg.active_scene.is_some(), Some(ICON_ROUTE));
                        egui::Popup::menu(&chip).show(|ui| {
                            for s in &view.cfg.scenes {
                                let active = view.cfg.active_scene.as_deref() == Some(s.name.as_str());
                                if widgets::menu_item(ui, Some(ICON_ROUTE), &s.name, "", active) {
                                    out.push(Action::ActivateScene(if active { None } else { Some(s.name.clone()) }));
                                }
                            }
                            if !view.cfg.scenes.is_empty() {
                                ui.separator();
                            }
                            if view.cfg.active_scene.is_some() && widgets::menu_item(ui, Some(ICON_CLOSE), "Deactivate", "", false) {
                                out.push(Action::ActivateScene(None));
                            }
                            if widgets::menu_item(ui, Some(ICON_SAVE), "Manage scenes…", "", false) {
                                out.push(Action::Goto(crate::app::Page::Scenes));
                            }
                        });
                    });
                });
        });
}
