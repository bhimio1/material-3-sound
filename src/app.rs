//! The window: navigation, pages, and everything that keeps the graph in
//! line with what the user asked for (the active scene, running effects).

use crate::backend::{Backend, Command, Event, Status, VolumeTarget};
use crate::config::Config;
use crate::effects::{Effect, EffectKind};
use crate::graph::{Graph, ObjId, Volume};
use crate::scenes::Scene;
use crate::ui::{effects as effects_page, kit, mixer, patchbay, scenes as scenes_page, settings};
use crate::units::{self, Cord, Unit};
use crate::volume;
use egui::{CornerRadius, Frame, Margin};
use material_3_ui::common;
use material_3_ui::icons::icons::*;
use material_3_ui::theme::ThemeManager;
use material_3_ui::toasts::{Toast, ToastKind, Toasts};
use material_3_ui::{keys, widgets};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Patchbay,
    Mixer,
    Scenes,
    Effects,
    Settings,
}

impl Page {
    pub const ALL: [Page; 5] = [Page::Patchbay, Page::Mixer, Page::Scenes, Page::Effects, Page::Settings];

    pub fn parse(s: &str) -> Option<Page> {
        Page::ALL.into_iter().find(|p| p.title().eq_ignore_ascii_case(s))
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::Patchbay => "Patchbay",
            Page::Mixer => "Mixer",
            Page::Scenes => "Scenes",
            Page::Effects => "Effects",
            Page::Settings => "Settings",
        }
    }

    fn icon(self) -> material_3_ui::icons::MaterialIcon {
        match self {
            Page::Patchbay => ICON_CABLE,
            Page::Mixer => ICON_TUNE,
            Page::Scenes => ICON_ROUTE,
            Page::Effects => ICON_EQUALIZER,
            Page::Settings => ICON_SETTINGS,
        }
    }
}

/// What pages ask for. The app carries it out, so the active scene and
/// effects stay in sync whichever page the change came from.
#[derive(Clone, Debug)]
pub enum Action {
    Connect(Vec<(ObjId, ObjId)>),
    Disconnect(Vec<ObjId>),
    Volume { node: ObjId, channels: Vec<f32>, mute: bool },
    SetDefault(ObjId),
    MoveCard(String, [f32; 2]),
    ResetLayout,
    ToggleChannels,
    Hide(String),
    Unhide(String),
    Goto(Page),
    /// Show a card's inspector in the patchbay.
    Inspect(String),
    EditEffect(String),
    AddEqualizer,
    AddBus,
    UpdateEffect(Effect),
    EnableEffect(String, bool),
    DeleteEffect(String),
    SaveScene(String),
    ApplyScene(String),
    ActivateScene(Option<String>),
    UpdateScene(String),
    DeleteScene(String),
    RenameScene(String, String),
    SceneExclusive(String, bool),
    RemoveScenePair { scene: String, out_node: String, in_node: String },
    UseDemo,
    Toast(String),
}

#[derive(Clone, Debug)]
pub enum ToastAction {
    Goto(Page),
}

/// A read-only look at the app's state for the pages.
pub struct View<'a> {
    pub graph: &'a Graph,
    pub units: &'a [Unit],
    pub cords: &'a [Cord],
    pub cfg: &'a Config,
    pub overrides: &'a HashMap<ObjId, (Volume, Instant)>,
    pub status: &'a Status,
    pub starting: &'a HashMap<String, Instant>,
    pub failed: &'a HashMap<String, u32>,
}

impl View<'_> {
    /// The volume to show: what the user is dragging, else the graph's.
    pub fn volume(&self, node: ObjId) -> Option<Volume> {
        if let Some((v, _)) = self.overrides.get(&node) {
            return Some(v.clone());
        }
        let n = self.graph.nodes.get(&node)?;
        self.graph.volume_of(n).cloned()
    }

    pub fn unit(&self, key: &str) -> Option<(usize, &Unit)> {
        self.units.iter().enumerate().find(|(_, u)| u.key == key)
    }

    /// Units fed by `i` (and feeding it).
    pub fn sends_of(&self, i: usize) -> Vec<usize> {
        self.cords.iter().filter(|c| c.from == i).map(|c| c.to).collect()
    }

    pub fn sources_of(&self, i: usize) -> Vec<usize> {
        self.cords.iter().filter(|c| c.to == i).map(|c| c.from).collect()
    }

    /// The title for a node name (in scenes): its card's, else the name.
    pub fn title_for(&self, node_name: &str) -> String {
        if let Some(e) = crate::effects::effect_for_node(&self.cfg.effects, node_name) {
            return e.name.clone();
        }
        self.graph.node_by_name(node_name).map(|n| n.title()).unwrap_or_else(|| node_name.to_string())
    }

    pub fn effect_running(&self, e: &Effect) -> bool {
        self.graph.node_by_name(&e.node_name()).is_some()
    }
}

pub struct App {
    pub cfg: Config,
    cfg_path: Option<PathBuf>,
    save_at: Option<Instant>,
    theme: ThemeManager,
    backend: Backend,
    status: Status,
    graph: Arc<Graph>,
    units: Vec<Unit>,
    cords: Vec<Cord>,
    page: Page,
    toasts: Toasts<ToastAction>,
    patch: patchbay::State,
    effects_ui: effects_page::State,
    scenes_ui: scenes_page::State,
    overrides: HashMap<ObjId, (Volume, Instant)>,
    enforce_at: Option<Instant>,
    attempts: HashMap<(ObjId, ObjId), (u32, Instant)>,
    /// Links an exclusive scene removed, by ports, to notice a fight.
    removals: HashMap<(ObjId, ObjId), (u32, Instant)>,
    warned: HashSet<(ObjId, ObjId)>,
    starting: HashMap<String, Instant>,
    failed: HashMap<String, u32>,
    touched: HashSet<String>,
    restart_at: HashMap<String, Instant>,
    /// Wiring to put back after an effect restarts, and until when to try.
    relink: Vec<(Scene, Instant)>,
    graphs_seen: u64,
    /// When the backend connected: effects start once the graph settles.
    connected_at: Option<Instant>,
}

impl App {
    pub fn new(ctx: &egui::Context, demo: bool, page: Page) -> Self {
        let (loaded, error) = Config::load();
        let theme = ThemeManager::new(ctx, &loaded.appearance);
        let (cfg, cfg_path, backend) = if demo {
            (crate::demo::config(&loaded), None, Backend::demo(Some(ctx.clone())))
        } else {
            (loaded, Some(crate::config::path()), Backend::pipewire(Some(ctx.clone())))
        };
        let mut app = Self {
            cfg,
            cfg_path,
            save_at: None,
            theme,
            backend,
            status: Status::Connecting,
            graph: Arc::new(Graph::default()),
            units: Vec::new(),
            cords: Vec::new(),
            page,
            toasts: Toasts::default(),
            patch: patchbay::State::default(),
            effects_ui: effects_page::State::default(),
            scenes_ui: scenes_page::State::default(),
            overrides: HashMap::new(),
            enforce_at: None,
            attempts: HashMap::new(),
            removals: HashMap::new(),
            warned: HashSet::new(),
            starting: HashMap::new(),
            failed: HashMap::new(),
            touched: HashSet::new(),
            restart_at: HashMap::new(),
            relink: Vec::new(),
            graphs_seen: 0,
            connected_at: None,
        };
        if let Some(e) = error {
            app.toasts.error(e);
        }
        if let Some(e) = app.theme.take_error() {
            app.toasts.error(e);
        }
        app
    }

    fn view(&self) -> View<'_> {
        View {
            graph: &self.graph,
            units: &self.units,
            cords: &self.cords,
            cfg: &self.cfg,
            overrides: &self.overrides,
            status: &self.status,
            starting: &self.starting,
            failed: &self.failed,
        }
    }

    fn dirty(&mut self) {
        if self.cfg_path.is_some() {
            self.save_at.get_or_insert(Instant::now() + Duration::from_millis(800));
        }
    }

    fn save_now(&mut self) {
        self.save_at = None;
        if let Some(path) = &self.cfg_path {
            if let Err(e) = self.cfg.save_to(path) {
                self.toasts.error(format!("Couldn't save settings: {e}"));
            }
        }
    }

    fn rebuild(&mut self) {
        self.units = units::build(&self.graph, self.cfg.show_other);
        for u in &mut self.units {
            if let Some(name) = self.graph.nodes.get(&u.main).map(|n| n.name.clone()) {
                if let Some(e) = crate::effects::effect_for_node(&self.cfg.effects, &name) {
                    u.title = e.name.clone();
                    u.subtitle = match &e.kind {
                        EffectKind::Equalizer(eq) => format!("Equalizer · {} bands", eq.bands.len()),
                        EffectKind::Bus(_) => "Mix bus".into(),
                    };
                }
            }
        }
        self.cords = units::cords(&self.graph, &self.units);
    }

    fn poll_backend(&mut self) {
        let mut new_graph = None;
        for ev in self.backend.events() {
            match ev {
                Event::Graph(g) => new_graph = Some(g),
                Event::Status(s) => {
                    self.connected_at = (s == Status::Connected).then(Instant::now);
                    if let Status::Failed(reason) = &s {
                        if self.status == Status::Connected {
                            self.toasts.error(format!("Lost PipeWire: {reason}"));
                        }
                    }
                    self.status = s;
                }
                Event::Error(e) => self.toasts.error(e),
            }
        }
        if let Some(g) = new_graph {
            self.graph = g;
            self.graphs_seen += 1;
            self.rebuild();
            let now = Instant::now();
            self.overrides.retain(|id, (v, at)| {
                let fresh = now.duration_since(*at) < Duration::from_millis(1500);
                let reported = self.graph.nodes.get(id).and_then(|n| self.graph.volume_of(n));
                fresh && reported.is_none_or(|r| r.mute != v.mute || r.channels.iter().zip(&v.channels).any(|(a, b)| (a - b).abs() > 1e-3))
            });
            self.enforce_at = Some(now + Duration::from_millis(350));
        }
    }

    /// Keep enabled effects running, restart equalizers whose bands
    /// changed shape, and keep the active scene connected.
    fn housekeeping(&mut self) {
        if self.status != Status::Connected || self.graphs_seen == 0 {
            return;
        }
        let now = Instant::now();
        // Restarts that are due: remember the wiring, then stop.
        let due: Vec<String> = self.restart_at.iter().filter(|(_, at)| now >= **at).map(|(id, _)| id.clone()).collect();
        for id in due {
            self.restart_at.remove(&id);
            if let Some(e) = self.cfg.effects.iter().find(|e| e.id == id).cloned() {
                self.remember_wiring(&e);
                self.stop_effect(&e);
                self.touched.insert(id);
            }
        }
        // Start enabled effects that aren't running, once the graph has
        // settled (the first listing can miss nodes still being described).
        let settled = self.connected_at.is_some_and(|t| now.duration_since(t) > Duration::from_millis(1500));
        let mut starts = Vec::new();
        for e in self.cfg.effects.iter().filter(|e| e.enabled && settled) {
            if !(self.cfg.restore_on_start || self.touched.contains(&e.id)) || self.restart_at.contains_key(&e.id) {
                continue;
            }
            if self.graph.node_by_name(&e.node_name()).is_some() {
                self.starting.remove(&e.id);
                self.failed.remove(&e.id);
                continue;
            }
            if self.starting.get(&e.id).is_some_and(|t| now.duration_since(*t) < Duration::from_secs(8)) {
                continue;
            }
            if self.failed.get(&e.id).copied().unwrap_or(0) >= 3 {
                continue;
            }
            starts.push(e.clone());
        }
        for e in starts {
            if self.starting.contains_key(&e.id) {
                *self.failed.entry(e.id.clone()).or_default() += 1;
            }
            self.starting.insert(e.id.clone(), now);
            self.backend.send(Command::StartEffect(e));
        }
        // The active scene, and wiring waiting for restarted effects.
        if self.enforce_at.is_some_and(|t| now >= t) {
            self.enforce_at = None;
            self.relink.retain(|(_, until)| now < *until);
            let mut scenes: Vec<Scene> = self.relink.iter().map(|(s, _)| s.clone()).collect();
            if let Some(s) = self.cfg.active_scene.as_deref().and_then(|n| self.cfg.scene(n)) {
                scenes.push(s.clone());
            }
            for s in scenes {
                let mut plan = s.reconcile(&self.graph);
                // Back off when something (usually WirePlumber) keeps undoing
                // a change, rather than fighting it forever.
                let mut gave_up = Vec::new();
                let mut allow = |map: &mut HashMap<(ObjId, ObjId), (u32, Instant)>, pair: (ObjId, ObjId)| {
                    let e = map.entry(pair).or_insert((0, now));
                    if now.duration_since(e.1) > Duration::from_secs(20) {
                        *e = (0, now);
                    }
                    e.0 += 1;
                    if e.0 > 4 {
                        gave_up.push(pair);
                    }
                    e.0 <= 4
                };
                plan.connect.retain(|pair| allow(&mut self.attempts, *pair));
                let graph = self.graph.clone();
                plan.disconnect.retain(|id| graph.links.get(id).is_none_or(|l| allow(&mut self.removals, (l.out_port, l.in_port))));
                for pair in gave_up {
                    if self.warned.insert(pair) {
                        let name = |port: ObjId| {
                            graph.ports.get(&port).and_then(|p| graph.nodes.get(&p.node)).map(|n| n.title()).unwrap_or_default()
                        };
                        self.toasts.error(format!(
                            "Something keeps changing the cord from {} to {}, so “{}” stopped enforcing it.",
                            name(pair.0),
                            name(pair.1),
                            s.name
                        ));
                    }
                }
                if !plan.disconnect.is_empty() {
                    self.backend.send(Command::Disconnect(plan.disconnect));
                }
                if !plan.connect.is_empty() {
                    self.backend.send(Command::Connect(plan.connect));
                }
            }
        }
    }

    /// Save an effect's cords by name before it restarts, to put them back.
    fn remember_wiring(&mut self, e: &Effect) {
        let names: Vec<String> = std::iter::once(e.node_name()).chain(e.output_node_name()).collect();
        let all = Scene::capture("", &self.graph, false);
        let into: Vec<_> = all.links.iter().filter(|l| names.contains(&l.in_node)).cloned().collect();
        let out: Vec<_> = all.links.iter().filter(|l| names.contains(&l.out_node)).cloned().collect();
        let until = Instant::now() + Duration::from_secs(15);
        if !into.is_empty() {
            self.relink.push((Scene { name: e.name.clone(), links: into, exclusive: false }, until));
        }
        if !out.is_empty() {
            // Exclusive, so WirePlumber's link to the default device goes.
            self.relink.push((Scene { name: e.name.clone(), links: out, exclusive: true }, until));
        }
    }

    fn stop_effect(&mut self, e: &Effect) {
        let names: Vec<String> = std::iter::once(e.node_name()).chain(e.output_node_name()).collect();
        let nodes: Vec<ObjId> = self.graph.nodes.values().filter(|n| names.contains(&n.name)).map(|n| n.id).collect();
        if nodes.is_empty() && !matches!(e.kind, EffectKind::Equalizer(_)) {
            return;
        }
        self.backend.send(Command::StopEffect { id: e.id.clone(), nodes });
    }

    fn unique_name(&self, base: &str) -> String {
        (1..)
            .map(|i| if i == 1 { base.to_string() } else { format!("{base} {i}") })
            .find(|n| !self.cfg.effects.iter().any(|e| &e.name == n))
            .unwrap_or_else(|| base.to_string())
    }

    fn handle(&mut self, a: Action) {
        match a {
            Action::Connect(pairs) => {
                if pairs.is_empty() {
                    return;
                }
                let g = self.graph.clone();
                if let Some(s) = self.cfg.active_scene_mut() {
                    s.add(&g, &pairs);
                    self.dirty();
                }
                self.backend.send(Command::Connect(pairs));
            }
            Action::Disconnect(ids) => {
                if ids.is_empty() {
                    return;
                }
                let g = self.graph.clone();
                if let Some(s) = self.cfg.active_scene_mut() {
                    s.remove(&g, &ids);
                    self.dirty();
                }
                self.backend.send(Command::Disconnect(ids));
            }
            Action::Volume { node, channels, mute } => {
                let Some(n) = self.graph.nodes.get(&node) else { return };
                let target = match self.graph.route_of(n) {
                    Some((dev, r)) => VolumeTarget::Route { device: dev.id, index: r.index, route_device: r.device },
                    None => VolumeTarget::Node(node),
                };
                let map = self.graph.volume_of(n).map(|v| v.map.clone()).unwrap_or_default();
                self.overrides.insert(node, (Volume { channels: channels.clone(), mute, map }, Instant::now()));
                self.backend.send(Command::SetVolume { target, channels, mute });
            }
            Action::SetDefault(node) => {
                if let Some(n) = self.graph.nodes.get(&node) {
                    self.backend.send(Command::SetDefault { sink: n.is_sink(), name: n.name.clone() });
                }
            }
            Action::MoveCard(key, pos) => {
                self.cfg.positions.insert(key, pos);
                self.dirty();
            }
            Action::ResetLayout => {
                self.cfg.positions.clear();
                self.patch.fit = true;
                self.dirty();
            }
            Action::ToggleChannels => {
                self.cfg.show_channels = !self.cfg.show_channels;
                self.dirty();
            }
            Action::Hide(key) => {
                if self.patch.selected.as_deref() == Some(key.as_str()) {
                    self.patch.selected = None;
                }
                self.cfg.hidden.insert(key);
                self.dirty();
            }
            Action::Unhide(key) => {
                self.cfg.hidden.remove(&key);
                self.dirty();
            }
            Action::Goto(p) => self.page = p,
            Action::Inspect(key) => {
                self.patch.selected = Some(key);
                self.page = Page::Patchbay;
            }
            Action::EditEffect(id) => {
                self.effects_ui.open = Some(id);
                self.page = Page::Effects;
            }
            Action::AddEqualizer | Action::AddBus => {
                let e = if matches!(a, Action::AddEqualizer) {
                    Effect::new_equalizer(&self.unique_name("Equalizer"))
                } else {
                    Effect::new_bus(&self.unique_name("Mix bus"))
                };
                self.touched.insert(e.id.clone());
                self.effects_ui.open = Some(e.id.clone());
                let mut t = Toast::new(format!("Added {}. Draw cords to it in the patchbay.", e.name), ToastKind::Success);
                t.action = Some(("Patchbay".into(), ToastAction::Goto(Page::Patchbay)));
                self.toasts.push(t);
                self.cfg.effects.push(e);
                self.dirty();
            }
            Action::UpdateEffect(new) => {
                let Some(slot) = self.cfg.effects.iter_mut().find(|e| e.id == new.id) else { return };
                let old = std::mem::replace(slot, new.clone());
                self.dirty();
                let (EffectKind::Equalizer(a), EffectKind::Equalizer(b)) = (&old.kind, &new.kind) else { return };
                let Some(node) = self.graph.node_by_name(&new.node_name()) else { return };
                let live_bands = node.controls.iter().filter(|(k, _)| k.starts_with("band") && k.ends_with(":Freq")).count();
                if a.same_shape(b) && live_bands == b.bands.len() {
                    let before = a.controls();
                    let changed: Vec<(String, f32)> = b.controls().into_iter().filter(|c| !before.contains(c)).collect();
                    if !changed.is_empty() {
                        self.backend.send(Command::SetControls { node: node.id, controls: changed });
                    }
                } else {
                    self.restart_at.insert(new.id.clone(), Instant::now() + Duration::from_millis(600));
                }
            }
            Action::EnableEffect(id, on) => {
                let Some(e) = self.cfg.effects.iter_mut().find(|e| e.id == id) else { return };
                e.enabled = on;
                let e = e.clone();
                self.touched.insert(id.clone());
                self.failed.remove(&id);
                self.starting.remove(&id);
                if !on {
                    self.stop_effect(&e);
                }
                self.dirty();
            }
            Action::DeleteEffect(id) => {
                if let Some(pos) = self.cfg.effects.iter().position(|e| e.id == id) {
                    let e = self.cfg.effects.remove(pos);
                    self.stop_effect(&e);
                    self.toasts.info(format!("Removed {}.", e.name));
                    self.dirty();
                }
            }
            Action::SaveScene(name) => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return;
                }
                let scene = Scene::capture(&name, &self.graph, true);
                let count = scene.pairs().len();
                match self.cfg.scenes.iter_mut().find(|s| s.name == name) {
                    Some(s) => s.links = scene.links,
                    None => self.cfg.scenes.push(scene),
                }
                self.toasts.success(format!("Saved “{name}” with {count} cords."));
                self.dirty();
            }
            Action::ApplyScene(name) => {
                if let Some(s) = self.cfg.scene(&name) {
                    let plan = s.reconcile(&self.graph);
                    let n = plan.connect.len() + plan.disconnect.len();
                    self.backend.send(Command::Disconnect(plan.disconnect));
                    self.backend.send(Command::Connect(plan.connect));
                    self.toasts.info(if n == 0 { format!("“{name}” is already connected.") } else { format!("Applied “{name}”.") });
                }
            }
            Action::ActivateScene(name) => {
                self.cfg.active_scene = name.clone();
                self.attempts.clear();
                self.removals.clear();
                self.warned.clear();
                self.enforce_at = Some(Instant::now());
                if let Some(n) = name {
                    self.toasts.info(format!("“{n}” is active: its cords reconnect when apps start, and cords you change are saved to it."));
                }
                self.dirty();
            }
            Action::UpdateScene(name) => {
                let links = Scene::capture(&name, &self.graph, true).links;
                if let Some(s) = self.cfg.scenes.iter_mut().find(|s| s.name == name) {
                    s.links = links;
                    self.toasts.success(format!("Updated “{name}” from the current wiring."));
                    self.dirty();
                }
            }
            Action::DeleteScene(name) => {
                self.cfg.scenes.retain(|s| s.name != name);
                if self.cfg.active_scene.as_deref() == Some(name.as_str()) {
                    self.cfg.active_scene = None;
                }
                self.dirty();
            }
            Action::RenameScene(old, new) => {
                let new = new.trim().to_string();
                if new.is_empty() || self.cfg.scenes.iter().any(|s| s.name == new) {
                    return;
                }
                if let Some(s) = self.cfg.scenes.iter_mut().find(|s| s.name == old) {
                    s.name = new.clone();
                }
                if self.cfg.active_scene.as_deref() == Some(old.as_str()) {
                    self.cfg.active_scene = Some(new);
                }
                self.dirty();
            }
            Action::SceneExclusive(name, on) => {
                if let Some(s) = self.cfg.scenes.iter_mut().find(|s| s.name == name) {
                    s.exclusive = on;
                    self.enforce_at = Some(Instant::now());
                    self.dirty();
                }
            }
            Action::RemoveScenePair { scene, out_node, in_node } => {
                if let Some(s) = self.cfg.scenes.iter_mut().find(|s| s.name == scene) {
                    s.remove_pair(&out_node, &in_node);
                    self.dirty();
                }
            }
            Action::UseDemo => self.use_demo(),
            Action::Toast(t) => self.toasts.info(t),
        }
    }

    fn use_demo(&mut self) {
        if self.backend.demo {
            return;
        }
        self.save_now();
        self.backend = Backend::demo(None);
        self.cfg = crate::demo::config(&self.cfg);
        self.cfg_path = None;
        self.graph = Arc::new(Graph::default());
        self.graphs_seen = 0;
        self.patch = patchbay::State::default();
        self.status = Status::Connecting;
        self.toasts.info("Showing a demo desktop. Nothing you change here touches your sound or settings.");
    }

    fn nav(&mut self, ui: &mut egui::Ui, out: &mut Vec<Action>) {
        let p = widgets::palette(ui);
        ui.horizontal(|ui| {
            widgets::icon(ui, ICON_GRAPHIC_EQ, 26.0, p.primary);
            ui.label(widgets::title_text("Sound", 20.0));
            if self.backend.demo {
                kit::pill(ui, "Demo", p.tertiary_container, p.on_tertiary_container);
            }
        });
        ui.add_space(14.0);
        for page in Page::ALL {
            let trailing = match page {
                Page::Patchbay => Some(self.cords.len().to_string()),
                Page::Scenes => self.cfg.active_scene.as_ref().map(|_| "●".to_string()),
                Page::Effects if !self.cfg.effects.is_empty() => Some(self.cfg.effects.len().to_string()),
                _ => None,
            };
            if widgets::nav_item(ui, page.icon(), page.title(), self.page == page, trailing.as_deref()).clicked() {
                self.page = page;
            }
        }
        // The default output, always at hand.
        let view = self.view();
        let default = view.units.iter().find(|u| u.is_default && view.graph.nodes.get(&u.main).is_some_and(|n| n.is_sink()));
        // Pushed to the bottom of the rail.
        let needed = if default.is_some() { 130.0 } else { 26.0 };
        ui.add_space((ui.available_height() - needed).max(12.0));
        if let Some(u) = default {
            if let Some(v) = view.volume(u.main) {
                Frame::new().fill(p.surface_container).corner_radius(CornerRadius::same(18)).inner_margin(Margin::same(12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        widgets::icon(ui, kit::unit_icon(view.graph, u), 18.0, p.primary);
                        let w = ui.available_width();
                        let g = widgets::truncated_galley(ui, &u.title, widgets::title_font(13.0), p.on_surface, w);
                        ui.label(g);
                    });
                    ui.horizontal(|ui| {
                        if kit::mute_button(ui, v.mute, 30.0).clicked() {
                            out.push(Action::Volume { node: u.main, channels: v.channels.clone(), mute: !v.mute });
                        }
                        let mut m = volume::master(&v);
                        let w = ui.available_width() - 4.0;
                        if kit::slider(ui, &mut m, view.cfg.max_volume(), w, p.primary, view.cfg.allow_boost.then_some(1.0)).changed() {
                            out.push(Action::Volume { node: u.main, channels: volume::with_master(&v, m), mute: v.mute });
                        }
                    });
                });
            }
        }
        let status = match &view.status {
            Status::Connected => match &view.graph.server {
                Some(s) => {
                    let rate = s.rate.map(|r| format!(" · {} kHz", r as f32 / 1000.0)).unwrap_or_default();
                    format!("PipeWire {}{rate}", s.version)
                }
                None => "Connected".into(),
            },
            Status::Connecting => "Connecting to PipeWire…".into(),
            Status::Failed(_) => "PipeWire unavailable".into(),
        };
        ui.add_space(6.0);
        ui.label(egui::RichText::new(status).size(12.0).color(p.on_surface_variant));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.theme.poll(&self.cfg.appearance);
        self.poll_backend();
        self.housekeeping();
        let ctx = ui.ctx().clone();
        // Ctrl+1…5 switch pages.
        for (i, page) in Page::ALL.into_iter().enumerate() {
            let key = [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5][i];
            if keys::take_key(&ctx, egui::Modifiers::COMMAND, key) {
                self.page = page;
            }
        }
        let p = self.theme.palette().clone();
        let mut actions = Vec::new();
        let surface = Frame::new().fill(p.surface);
        egui::Panel::left("nav")
            .default_size(232.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(surface.inner_margin(Margin::same(12)))
            .show(ui, |ui| self.nav(ui, &mut actions));
        egui::CentralPanel::default().frame(surface.inner_margin(Margin { left: 0, right: 12, top: 12, bottom: 12 })).show(ui, |ui| {
            Frame::new().fill(p.surface_container_low).corner_radius(CornerRadius::same(24)).show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                let view = View {
                    graph: &self.graph,
                    units: &self.units,
                    cords: &self.cords,
                    cfg: &self.cfg,
                    overrides: &self.overrides,
                    status: &self.status,
                    starting: &self.starting,
                    failed: &self.failed,
                };
                match self.page {
                    Page::Patchbay => patchbay::show(ui, &view, &mut self.patch, &mut actions),
                    Page::Mixer => mixer::show(ui, &view, &mut actions),
                    Page::Scenes => scenes_page::show(ui, &view, &mut self.scenes_ui, &mut actions),
                    Page::Effects => effects_page::show(ui, &view, &mut self.effects_ui, &mut actions),
                    Page::Settings => {
                        let before = self.cfg.clone();
                        let applied = settings::show(ui, &mut self.cfg, &self.theme, &view_status(&self.status, &self.graph, self.backend.demo, self.cfg_path.as_ref()), &mut actions);
                        if applied {
                            self.theme.apply(&self.cfg.appearance);
                        }
                        if self.cfg != before {
                            if self.cfg.show_other != before.show_other {
                                self.rebuild();
                            }
                            self.dirty();
                        }
                    }
                }
            });
        });
        for a in actions {
            self.handle(a);
        }
        for a in self.toasts.show(&ctx, &p, 20.0) {
            match a {
                ToastAction::Goto(page) => self.page = page,
            }
        }
        if self.save_at.is_some_and(|t| Instant::now() >= t) {
            self.save_now();
        }
        // Wake up for scheduled work.
        let next = [self.save_at, self.enforce_at, self.restart_at.values().min().copied()].into_iter().flatten().min();
        if let Some(t) = next {
            ctx.request_repaint_after(t.saturating_duration_since(Instant::now()));
        }
        if !self.starting.is_empty() || self.connected_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_now();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.palette().surface.to_normalized_gamma_f32()
    }
}

/// Facts for the About card.
pub struct AboutInfo {
    pub backend: String,
    pub server: String,
    pub config: String,
}

fn view_status(status: &Status, g: &Graph, demo: bool, path: Option<&PathBuf>) -> AboutInfo {
    AboutInfo {
        backend: if demo {
            "Demo (simulated)".into()
        } else {
            match status {
                Status::Connected => "PipeWire".into(),
                Status::Connecting => "Connecting…".into(),
                Status::Failed(e) => format!("Unavailable: {e}"),
            }
        },
        server: g
            .server
            .as_ref()
            .map(|s| {
                let mut t = s.version.clone();
                if let Some(r) = s.rate {
                    t.push_str(&format!(" · {r} Hz"));
                }
                if let Some(q) = s.quantum {
                    t.push_str(&format!(" · quantum {q}"));
                }
                t
            })
            .unwrap_or_else(|| "—".into()),
        config: path.map(|p| material_3_ui::sys::paths::tilde(p)).unwrap_or_else(|| "Not saved in the demo".into()),
    }
}

/// Status banner shown at the top of pages when PipeWire isn't reachable.
pub fn status_banner(ui: &mut egui::Ui, view: &View, out: &mut Vec<Action>) {
    if let Status::Failed(reason) = view.status {
        common::banner(ui, ICON_WARNING, &format!("Can't reach PipeWire: {reason} Retrying…"), true, |ui| {
            if widgets::button(ui, widgets::ButtonKind::Filled, Some(ICON_PLAY_ARROW), "Try the demo").clicked() {
                out.push(Action::UseDemo);
            }
        });
    }
}
