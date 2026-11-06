//! A simulated desktop for `--demo` (and for trying the app where PipeWire
//! isn't running): a few apps, devices, an equalizer and a mix bus, which
//! the app can rewire like the real thing.

use crate::backend::{Command, Event, Status, VolumeTarget};
use crate::config::Config;
use crate::effects::{Effect, EffectKind, Equalizer};
use crate::graph::*;
use crate::scenes::{Scene, SceneLink};
use std::sync::Arc;

pub struct Sim {
    pub g: Graph,
    next: ObjId,
}

const EQ_ID: &str = "demo1";
const BUS_ID: &str = "demo2";

impl Sim {
    fn id(&mut self) -> ObjId {
        self.next += 1;
        self.next
    }

    #[allow(clippy::too_many_arguments)]
    fn node(&mut self, name: &str, title: &str, class: &str, extra: &str, ins: &[&str], outs: &[&str], volume: f32) -> ObjId {
        let id = self.id();
        let stream = class.starts_with("Stream/");
        let channels = if ins.len() >= outs.len() { ins } else { outs };
        let channels: Vec<String> = channels.iter().map(|c| c.to_string()).collect();
        let node = Node {
            id,
            serial: id as u64,
            name: name.into(),
            description: (!stream).then(|| title.into()),
            nick: None,
            app: stream.then(|| title.into()),
            binary: None,
            icon_name: None,
            media_name: (stream && !extra.is_empty()).then(|| extra.into()),
            media_class: class.into(),
            role: None,
            state: NodeState::Running,
            volume: Some(Volume { channels: vec![volume; channels.len().max(1)], mute: false, map: channels.clone() }),
            device_id: (!stream && !name.starts_with("m3sound.")).then_some(1),
            profile_device: None,
            link_group: None,
            client_id: None,
            factory: None,
            api: (!stream && !name.starts_with("m3sound.")).then(|| "alsa".into()),
            virtual_: false,
            controls: Vec::new(),
        };
        let mut node = node;
        if !stream && !extra.is_empty() {
            node.nick = Some(extra.into());
        }
        self.g.nodes.insert(id, node);
        let monitor = class.starts_with("Audio/Sink");
        for (dir, list, prefix) in [(Direction::In, ins, if stream { "input" } else { "playback" }), (Direction::Out, outs, if monitor { "monitor" } else if stream { "output" } else { "capture" })] {
            for ch in list {
                let pid = self.id();
                self.g.ports.insert(
                    pid,
                    Port { id: pid, node: id, direction: dir, name: format!("{prefix}_{ch}"), channel: ch.to_string(), media: Media::Audio, monitor: monitor && dir == Direction::Out },
                );
            }
        }
        id
    }

    fn sink(&mut self, name: &str, title: &str, nick: &str, volume: f32) -> ObjId {
        self.node(name, title, "Audio/Sink", nick, &["FL", "FR"], &["FL", "FR"], volume)
    }

    /// Link every channel of `from` into `to`, as WirePlumber would.
    pub fn connect_nodes(&mut self, from: ObjId, to: ObjId) {
        let outs: Vec<Port> = self.g.ports_of(from, Direction::Out).into_iter().cloned().collect();
        let ins: Vec<Port> = self.g.ports_of(to, Direction::In).into_iter().cloned().collect();
        let pairs = plan_links(&outs.iter().collect::<Vec<_>>(), &ins.iter().collect::<Vec<_>>());
        self.link(&pairs);
    }

    fn link(&mut self, pairs: &[(ObjId, ObjId)]) {
        for &(o, i) in pairs {
            let (Some(op), Some(ip)) = (self.g.ports.get(&o).cloned(), self.g.ports.get(&i).cloned()) else { continue };
            if op.direction != Direction::Out || ip.direction != Direction::In || self.g.link_between(o, i).is_some() {
                continue;
            }
            let running = self.g.nodes.get(&op.node).is_some_and(|n| n.state == NodeState::Running);
            let id = self.id();
            self.g.links.insert(
                id,
                Link { id, out_node: op.node, out_port: o, in_node: ip.node, in_port: i, state: if running { LinkState::Active } else { LinkState::Paused } },
            );
        }
    }

    fn remove_node(&mut self, id: ObjId) {
        self.g.nodes.remove(&id);
        let ports: Vec<ObjId> = self.g.ports.values().filter(|p| p.node == id).map(|p| p.id).collect();
        for p in &ports {
            self.g.ports.remove(p);
        }
        self.g.links.retain(|_, l| l.out_node != id && l.in_node != id);
    }

    fn start_effect(&mut self, e: &Effect) {
        if self.g.node_by_name(&e.node_name()).is_some() {
            return;
        }
        match &e.kind {
            EffectKind::Bus(_) => {
                let id = self.sink(&e.node_name(), &e.name, "", 1.0);
                if let Some(n) = self.g.nodes.get_mut(&id) {
                    n.device_id = None;
                    n.api = None;
                    n.virtual_ = true;
                }
            }
            EffectKind::Equalizer(eq) => {
                let input = self.sink(&e.node_name(), &e.name, "", 1.0);
                let output = self.node(&e.output_node_name().unwrap_or_default(), &e.name, "Stream/Output/Audio", "", &[], &["FL", "FR"], 1.0);
                let group = format!("filter-chain-demo-{}", e.id);
                for (id, is_input) in [(input, true), (output, false)] {
                    if let Some(n) = self.g.nodes.get_mut(&id) {
                        n.link_group = Some(group.clone());
                        n.device_id = None;
                        n.api = None;
                        n.app = None;
                        n.description = Some(e.name.clone());
                        if is_input {
                            n.controls = eq.controls();
                        } else {
                            n.volume = None;
                        }
                    }
                }
                let default = self.g.defaults.sink.clone().and_then(|s| self.g.node_by_name(&s).map(|n| n.id));
                if let Some(d) = default {
                    self.connect_nodes(output, d);
                }
            }
        }
    }

    fn apply(&mut self, c: Command) {
        match c {
            Command::Connect(pairs) => self.link(&pairs),
            Command::Disconnect(ids) => {
                for id in ids {
                    self.g.links.remove(&id);
                }
            }
            Command::SetVolume { target: VolumeTarget::Node(id), channels, mute } => {
                if let Some(v) = self.g.nodes.get_mut(&id).and_then(|n| n.volume.as_mut()) {
                    v.channels = channels;
                    v.mute = mute;
                }
            }
            Command::SetVolume { .. } => {}
            Command::SetDefault { sink, name } => {
                if sink {
                    self.g.defaults.sink = Some(name.clone());
                    self.g.defaults.configured_sink = Some(name);
                } else {
                    self.g.defaults.source = Some(name.clone());
                    self.g.defaults.configured_source = Some(name);
                }
            }
            Command::SetControls { node, controls } => {
                if let Some(n) = self.g.nodes.get_mut(&node) {
                    for (k, v) in controls {
                        match n.controls.iter_mut().find(|(ck, _)| *ck == k) {
                            Some(c) => c.1 = v,
                            None => n.controls.push((k, v)),
                        }
                    }
                }
            }
            Command::StartEffect(e) => self.start_effect(&e),
            Command::StopEffect { nodes, .. } => {
                for n in nodes {
                    self.remove_node(n);
                }
            }
        }
    }
}

/// The demo's effects and scenes, matching [`desktop`].
pub fn config(base: &Config) -> Config {
    let mut c = base.clone();
    let mut eq = Effect::new_equalizer("Headphone EQ");
    eq.id = EQ_ID.into();
    eq.kind = EffectKind::Equalizer(Equalizer::preset("Bass boost").unwrap_or_default());
    let mut bus = Effect::new_bus("Stream mix");
    bus.id = BUS_ID.into();
    c.effects = vec![eq, bus];
    let link = |o: &str, op: &str, i: &str, ip: &str| SceneLink { out_node: o.into(), out_port: op.into(), in_node: i.into(), in_port: ip.into() };
    let stereo = |o: &str, op: &str, i: &str, ip: &str| {
        vec![link(o, &format!("{op}_FL"), i, &format!("{ip}_FL")), link(o, &format!("{op}_FR"), i, &format!("{ip}_FR"))]
    };
    c.scenes = vec![
        Scene {
            name: "Everyday".into(),
            links: [
                stereo("spotify", "output", "m3sound.eq.demo1", "playback"),
                stereo("m3sound.eq.demo1.out", "output", "bluez_output.headphones", "playback"),
                stereo("Firefox", "output", "alsa_output.speakers", "playback"),
            ]
            .concat(),
            exclusive: true,
        },
        Scene {
            name: "Streaming".into(),
            links: [
                stereo("spotify", "output", "m3sound.bus.demo2", "playback"),
                stereo("m3sound.bus.demo2", "monitor", "OBS", "input"),
                stereo("m3sound.bus.demo2", "monitor", "bluez_output.headphones", "playback"),
            ]
            .concat(),
            exclusive: false,
        },
    ];
    c.active_scene = None;
    c
}

/// A desktop with music, a browser, a voice chat and a recorder.
pub fn desktop() -> Sim {
    let mut s = Sim { g: Graph::default(), next: 30 };
    s.g.server = Some(Server { version: "1.4 (demo)".into(), rate: Some(48000), quantum: Some(1024) });
    let speakers = s.sink("alsa_output.speakers", "Built-in Speakers", "Laptop speakers", 0.55);
    let phones = s.sink("bluez_output.headphones", "Wireless Headphones", "Bluetooth", 0.35);
    let hdmi = s.sink("alsa_output.hdmi", "Monitor Speakers", "HDMI / DisplayPort", 0.8);
    let mic = s.node("alsa_input.mic", "Built-in Microphone", "Audio/Source", "Digital mic array", &[], &["FL", "FR"], 0.6);
    let usb = s.node("alsa_input.usb", "USB Microphone", "Audio/Source", "Podcast mic", &[], &["MONO"], 0.75);
    s.g.defaults = Defaults {
        metadata_id: None,
        sink: Some("bluez_output.headphones".into()),
        source: Some("alsa_input.usb".into()),
        configured_sink: Some("bluez_output.headphones".into()),
        configured_source: Some("alsa_input.usb".into()),
    };
    let spotify = s.node("spotify", "Spotify", "Stream/Output/Audio", "Lo-fi beats to study to", &[], &["FL", "FR"], 0.7);
    let firefox = s.node("Firefox", "Firefox", "Stream/Output/Audio", "Synthwave mix — YouTube", &[], &["FL", "FR"], 0.9);
    let discord = s.node("Discord", "Discord", "Stream/Output/Audio", "Voice chat", &[], &["FL", "FR"], 1.0);
    let mpv = s.node("mpv", "mpv", "Stream/Output/Audio", "Movie night.mkv", &[], &["FL", "FR"], 1.0);
    let obs = s.node("OBS", "OBS Studio", "Stream/Input/Audio", "Mic/Aux", &["FL", "FR"], &[], 1.0);
    let voice_in = s.node("Discord-mic", "Discord", "Stream/Input/Audio", "Voice input", &["MONO"], &[], 1.0);
    for (id, role) in [(spotify, "Music"), (firefox, "Movie"), (discord, "Communication"), (mpv, "Movie")] {
        if let Some(n) = s.g.nodes.get_mut(&id) {
            n.role = Some(role.into());
        }
    }
    if let Some(n) = s.g.nodes.get_mut(&mpv) {
        n.state = NodeState::Idle;
    }
    for e in config(&Config::default()).effects {
        s.start_effect(&e);
    }
    let eq = s.g.node_by_name("m3sound.eq.demo1").map(|n| n.id).unwrap_or_default();
    let bus = s.g.node_by_name("m3sound.bus.demo2").map(|n| n.id).unwrap_or_default();
    // The equalizer started on the default device; the rest is the user's wiring.
    s.connect_nodes(spotify, eq);
    s.connect_nodes(spotify, bus);
    s.connect_nodes(bus, obs);
    s.connect_nodes(firefox, speakers);
    s.connect_nodes(discord, phones);
    s.connect_nodes(mpv, hdmi);
    s.connect_nodes(mic, obs);
    s.connect_nodes(usb, voice_in);
    s
}

pub fn run(rx: flume::Receiver<Command>, events: flume::Sender<Event>, ctx: Option<egui::Context>) {
    let mut sim = desktop();
    let send = |sim: &Sim| {
        let ok = events.send(Event::Graph(Arc::new(sim.g.clone()))).is_ok();
        if let Some(c) = &ctx {
            c.request_repaint();
        }
        ok
    };
    let _ = events.send(Event::Status(Status::Connected));
    if !send(&sim) {
        return;
    }
    while let Ok(c) = rx.recv() {
        sim.apply(c);
        for c in rx.try_iter() {
            sim.apply(c);
        }
        if !send(&sim) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units;

    #[test]
    fn the_demo_desktop_is_wired_and_scenes_apply() {
        let mut sim = desktop();
        let u = units::build(&sim.g, false);
        assert!(u.iter().any(|u| u.kind == Kind::Effect && u.title == "Headphone EQ"));
        assert!(u.iter().any(|u| u.kind == Kind::Virtual && u.subtitle == "Mix bus"));
        assert!(u.iter().any(|u| u.kind == Kind::Output && u.is_default));
        let cords = units::cords(&sim.g, &u);
        assert!(cords.len() >= 8, "{}", cords.len());
        assert!(cords.iter().any(|c| !c.active), "mpv is idle");

        let cfg = config(&Config::default());
        let everyday = cfg.scene("Everyday").unwrap();
        let plan = everyday.reconcile(&sim.g);
        // Spotify's cord to the bus goes (exclusive); Firefox gets the speakers it has.
        assert!(!plan.disconnect.is_empty());
        sim.apply(Command::Disconnect(plan.disconnect.clone()));
        sim.apply(Command::Connect(plan.connect.clone()));
        assert!(everyday.reconcile(&sim.g).is_empty());

        // Stopping and starting the equalizer.
        let eq_nodes: Vec<ObjId> = sim.g.nodes.values().filter(|n| n.name.starts_with("m3sound.eq.")).map(|n| n.id).collect();
        assert_eq!(eq_nodes.len(), 2);
        sim.apply(Command::StopEffect { id: EQ_ID.into(), nodes: eq_nodes });
        assert!(sim.g.node_by_name("m3sound.eq.demo1").is_none());
        sim.apply(Command::StartEffect(cfg.effects[0].clone()));
        assert!(sim.g.node_by_name("m3sound.eq.demo1.out").is_some());
        assert!(!everyday.reconcile(&sim.g).connect.is_empty(), "the scene brings Spotify back to the new equalizer");
    }
}
