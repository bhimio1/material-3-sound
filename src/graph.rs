//! The PipeWire graph as the app sees it: nodes, their ports, the links
//! between ports, devices (for hardware volume routes), and the default
//! devices from the "default" metadata.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type ObjId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    In,
    Out,
}

/// What flows through a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Media {
    Audio,
    Midi,
    Video,
    Other,
}

/// How a node shows up in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Kind {
    /// An app playing sound (`Stream/Output/Audio`).
    Player,
    /// A microphone or other capture device (`Audio/Source`).
    Input,
    /// A filter (equalizer, loopback) with an input and an output side.
    Effect,
    /// A virtual device: a mix bus, null sink or virtual source.
    Virtual,
    /// Speakers, headphones, HDMI… (`Audio/Sink`).
    Output,
    /// An app recording sound (`Stream/Input/Audio`).
    Recorder,
    /// MIDI, video and anything else.
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Player => "App",
            Kind::Input => "Input",
            Kind::Effect => "Effect",
            Kind::Virtual => "Virtual device",
            Kind::Output => "Output",
            Kind::Recorder => "Recording",
            Kind::Other => "Other",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeState {
    Running,
    Idle,
    #[default]
    Suspended,
    Creating,
    Error,
}

/// Channel volumes in PipeWire's linear scale (cubed from what sliders
/// show), the mute flag and the channel names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Volume {
    pub channels: Vec<f32>,
    pub mute: bool,
    pub map: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub id: ObjId,
    pub serial: u64,
    /// `node.name`: stable across restarts for devices and most apps.
    pub name: String,
    pub description: Option<String>,
    pub nick: Option<String>,
    pub app: Option<String>,
    pub binary: Option<String>,
    pub icon_name: Option<String>,
    pub media_name: Option<String>,
    pub media_class: String,
    pub role: Option<String>,
    pub state: NodeState,
    pub volume: Option<Volume>,
    pub device_id: Option<ObjId>,
    /// `card.profile.device`: which of the device's routes drives this node.
    pub profile_device: Option<i64>,
    pub link_group: Option<String>,
    pub client_id: Option<ObjId>,
    pub factory: Option<String>,
    pub api: Option<String>,
    pub virtual_: bool,
    /// A filter-chain's control values, `("band1:Gain", 3.0)`.
    pub controls: Vec<(String, f32)>,
}

impl Node {
    /// What people call it: the device description, the app name, or the
    /// node name.
    pub fn title(&self) -> String {
        let pick = if self.is_stream() {
            self.app.as_ref().or(self.description.as_ref()).or(self.nick.as_ref())
        } else {
            self.description.as_ref().or(self.nick.as_ref())
        };
        pick.filter(|s| !s.trim().is_empty()).cloned().unwrap_or_else(|| self.name.clone())
    }

    pub fn is_stream(&self) -> bool {
        self.media_class.starts_with("Stream/")
    }

    pub fn is_sink(&self) -> bool {
        self.media_class.starts_with("Audio/Sink")
    }

    pub fn is_source(&self) -> bool {
        self.media_class.starts_with("Audio/Source")
    }

    /// The kind before effects are merged (see [`crate::units`]).
    pub fn kind(&self) -> Kind {
        let c = self.media_class.as_str();
        let ours = self.name.starts_with(crate::effects::NODE_PREFIX);
        match c {
            "Stream/Output/Audio" => Kind::Player,
            "Stream/Input/Audio" => Kind::Recorder,
            _ if c.starts_with("Audio/Sink") || c == "Audio/Duplex" => {
                if ours && self.name.contains(".eq.") {
                    Kind::Effect
                } else if self.virtual_ || ours || self.device_id.is_none() {
                    Kind::Virtual
                } else {
                    Kind::Output
                }
            }
            _ if c.starts_with("Audio/Source") => {
                if c.ends_with("/Virtual") || self.virtual_ || (self.device_id.is_none() && self.api.is_none()) {
                    Kind::Virtual
                } else {
                    Kind::Input
                }
            }
            _ => Kind::Other,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub id: ObjId,
    pub node: ObjId,
    pub direction: Direction,
    /// `port.name`, e.g. `playback_FL`.
    pub name: String,
    /// `audio.channel`, e.g. `FL` (empty when unknown).
    pub channel: String,
    pub media: Media,
    pub monitor: bool,
}

impl Port {
    /// A short label: the channel, else the port name.
    pub fn label(&self) -> &str {
        if self.channel.is_empty() {
            &self.name
        } else {
            &self.channel
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Active,
    Paused,
    Init,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub id: ObjId,
    pub out_node: ObjId,
    pub out_port: ObjId,
    pub in_node: ObjId,
    pub in_port: ObjId,
    pub state: LinkState,
}

/// A device route (e.g. "Headphones" on a sound card) and its volume.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub index: i64,
    pub device: i64,
    pub direction: Direction,
    pub description: String,
    pub volume: Option<Volume>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: ObjId,
    pub name: String,
    pub description: String,
    pub routes: Vec<Route>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Client {
    pub id: ObjId,
    pub pid: Option<u32>,
    pub app: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Server {
    pub version: String,
    pub rate: Option<u32>,
    pub quantum: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Defaults {
    pub metadata_id: Option<ObjId>,
    pub sink: Option<String>,
    pub source: Option<String>,
    pub configured_sink: Option<String>,
    pub configured_source: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub nodes: BTreeMap<ObjId, Node>,
    pub ports: BTreeMap<ObjId, Port>,
    pub links: BTreeMap<ObjId, Link>,
    pub devices: BTreeMap<ObjId, Device>,
    pub clients: BTreeMap<ObjId, Client>,
    pub defaults: Defaults,
    pub server: Option<Server>,
}

/// One object from the graph, as a monitor reports it.
#[derive(Clone, Debug, PartialEq)]
pub enum Object {
    Node(Box<Node>),
    Port(Port),
    Link(Link),
    Device(Device),
    Client(Client),
    /// The "default" metadata.
    Defaults(Defaults),
    Server(Server),
    /// Anything else (modules, factories, other metadata).
    Other(ObjId),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    Upsert(Object),
    Remove(ObjId),
}

impl Graph {
    pub fn apply(&mut self, updates: impl IntoIterator<Item = Update>) {
        for u in updates {
            match u {
                Update::Upsert(o) => self.upsert(o),
                Update::Remove(id) => self.remove(id),
            }
        }
    }

    fn upsert(&mut self, o: Object) {
        match o {
            Object::Node(n) => {
                self.nodes.insert(n.id, *n);
            }
            Object::Port(p) => {
                self.ports.insert(p.id, p);
            }
            Object::Link(l) => {
                self.links.insert(l.id, l);
            }
            Object::Device(d) => {
                self.devices.insert(d.id, d);
            }
            Object::Client(c) => {
                self.clients.insert(c.id, c);
            }
            Object::Defaults(d) => self.defaults = d,
            Object::Server(s) => self.server = Some(s),
            Object::Other(_) => {}
        }
    }

    fn remove(&mut self, id: ObjId) {
        self.nodes.remove(&id);
        self.ports.remove(&id);
        self.links.remove(&id);
        self.devices.remove(&id);
        self.clients.remove(&id);
        if self.defaults.metadata_id == Some(id) {
            self.defaults = Defaults::default();
        }
    }

    /// The node a name refers to (the first, when several share it).
    pub fn node_by_name(&self, name: &str) -> Option<&Node> {
        self.nodes.values().find(|n| n.name == name)
    }

    /// A node's ports in one direction, in channel order.
    pub fn ports_of(&self, node: ObjId, dir: Direction) -> Vec<&Port> {
        let mut v: Vec<&Port> = self.ports.values().filter(|p| p.node == node && p.direction == dir).collect();
        v.sort_by_key(|p| (channel_rank(&p.channel), p.name.clone()));
        v
    }

    pub fn link_between(&self, out_port: ObjId, in_port: ObjId) -> Option<&Link> {
        self.links.values().find(|l| l.out_port == out_port && l.in_port == in_port)
    }

    pub fn is_default(&self, node: &Node) -> bool {
        let target = if node.is_sink() {
            &self.defaults.sink
        } else if node.is_source() {
            &self.defaults.source
        } else {
            return false;
        };
        target.as_deref() == Some(node.name.as_str())
    }

    /// The device route behind a hardware node, whose volume is the one
    /// to show and set (WirePlumber saves it per route).
    pub fn route_of(&self, node: &Node) -> Option<(&Device, &Route)> {
        let dev = self.devices.get(&node.device_id?)?;
        let pd = node.profile_device?;
        let route = dev.routes.iter().find(|r| r.device == pd && r.volume.is_some())?;
        Some((dev, route))
    }

    /// The volume to show for a node: its route's, else its own.
    pub fn volume_of<'a>(&'a self, node: &'a Node) -> Option<&'a Volume> {
        self.route_of(node).and_then(|(_, r)| r.volume.as_ref()).or(node.volume.as_ref())
    }

    /// The process that owns a node, from its client.
    #[cfg(test)]
    pub fn pid_of(&self, node: &Node) -> Option<u32> {
        self.clients.get(&node.client_id?)?.pid
    }
}

/// Sort key for channel names: front left/right first, like a mixer.
pub fn channel_rank(ch: &str) -> usize {
    const ORDER: [&str; 12] = ["MONO", "FL", "FR", "FC", "LFE", "SL", "SR", "RL", "RR", "RC", "FLC", "FRC"];
    ORDER.iter().position(|c| *c == ch).unwrap_or_else(|| {
        // AUX0, AUX1… keep their numeric order after the named ones.
        ch.strip_prefix("AUX").and_then(|n| n.parse::<usize>().ok()).map_or(1000, |n| 100 + n)
    })
}

/// Which port pairs to link so `outs` play into `ins` channel by channel:
/// same channel names pair up, mono fans out to every input (and every
/// output folds into a mono input), otherwise ports pair in order.
pub fn plan_links(outs: &[&Port], ins: &[&Port]) -> Vec<(ObjId, ObjId)> {
    if outs.is_empty() || ins.is_empty() {
        return Vec::new();
    }
    let mono = |ps: &[&Port]| ps.len() == 1;
    if mono(outs) {
        return ins.iter().map(|i| (outs[0].id, i.id)).collect();
    }
    if mono(ins) {
        return outs.iter().map(|o| (o.id, ins[0].id)).collect();
    }
    let mut pairs = Vec::new();
    let named = outs.iter().any(|o| !o.channel.is_empty() && ins.iter().any(|i| i.channel == o.channel));
    if named {
        for o in outs {
            if let Some(i) = ins.iter().find(|i| !o.channel.is_empty() && i.channel == o.channel) {
                pairs.push((o.id, i.id));
            }
        }
        // Inputs nobody feeds (e.g. stereo into 5.1's FC) stay empty, which
        // is what WirePlumber does too.
        return pairs;
    }
    for (o, i) in outs.iter().zip(ins.iter()) {
        pairs.push((o.id, i.id));
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(id: ObjId, ch: &str, dir: Direction) -> Port {
        Port { id, node: 1, direction: dir, name: format!("p_{ch}"), channel: ch.into(), media: Media::Audio, monitor: false }
    }

    #[test]
    fn plans_channel_pairs() {
        let (fl, fr) = (port(1, "FL", Direction::Out), port(2, "FR", Direction::Out));
        let (ifr, ifl) = (port(11, "FR", Direction::In), port(10, "FL", Direction::In));
        assert_eq!(plan_links(&[&fl, &fr], &[&ifr, &ifl]), vec![(1, 10), (2, 11)]);
        let mono_in = port(20, "MONO", Direction::In);
        assert_eq!(plan_links(&[&fl, &fr], &[&mono_in]), vec![(1, 20), (2, 20)]);
        let mono_out = port(3, "MONO", Direction::Out);
        assert_eq!(plan_links(&[&mono_out], &[&ifl, &ifr]), vec![(3, 10), (3, 11)]);
        let (a0, a1) = (port(30, "AUX0", Direction::In), port(31, "AUX1", Direction::In));
        assert_eq!(plan_links(&[&fl, &fr], &[&a0, &a1]), vec![(1, 30), (2, 31)]);
        assert!(plan_links(&[], &[&a0]).is_empty());
    }

    #[test]
    fn channel_order() {
        let mut chans = vec!["FR", "AUX1", "FL", "LFE", "AUX0", "X"];
        chans.sort_by_key(|c| channel_rank(c));
        assert_eq!(chans, vec!["FL", "FR", "LFE", "AUX0", "AUX1", "X"]);
    }

    #[test]
    fn kinds_from_media_class() {
        let mut n = Node { media_class: "Stream/Output/Audio".into(), ..Default::default() };
        assert_eq!(n.kind(), Kind::Player);
        n.media_class = "Audio/Sink".into();
        assert_eq!(n.kind(), Kind::Virtual, "a sink without a device is virtual");
        n.device_id = Some(4);
        assert_eq!(n.kind(), Kind::Output);
        n.name = "m3sound.eq.abc".into();
        assert_eq!(n.kind(), Kind::Effect);
        let mic = Node { media_class: "Audio/Source".into(), device_id: Some(3), api: Some("alsa".into()), ..Default::default() };
        assert_eq!(mic.kind(), Kind::Input);
        let vs = Node { media_class: "Audio/Source/Virtual".into(), ..Default::default() };
        assert_eq!(vs.kind(), Kind::Virtual);
    }
}
