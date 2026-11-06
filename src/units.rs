//! What the patchbay draws: "units" (one card per app, device or effect,
//! with an equalizer's two PipeWire nodes merged into one card) and the
//! cords between them (all the port links between two units, bundled).

use crate::graph::*;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    /// Stable across restarts: where the card goes, and whether it's hidden.
    pub key: String,
    pub kind: Kind,
    /// The node whose volume the card controls (an effect's input side).
    pub main: ObjId,
    pub nodes: Vec<ObjId>,
    pub title: String,
    pub subtitle: String,
    pub inputs: Vec<ObjId>,
    pub outputs: Vec<ObjId>,
    /// The outputs are a sink's monitor (what it's playing).
    pub monitor_out: bool,
    pub is_default: bool,
    pub running: bool,
    pub media: Media,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cord {
    pub from: usize,
    pub to: usize,
    pub links: Vec<ObjId>,
    pub active: bool,
    pub monitor: bool,
}

/// Build the cards. `include_other` adds MIDI and video nodes.
pub fn build(g: &Graph, include_other: bool) -> Vec<Unit> {
    let mut groups: HashMap<&str, Vec<ObjId>> = HashMap::new();
    for n in g.nodes.values() {
        if let Some(lg) = &n.link_group {
            groups.entry(lg.as_str()).or_default().push(n.id);
        }
    }
    // Pairs of (input side, output side) to merge.
    let mut merged_into: HashMap<ObjId, ObjId> = HashMap::new();
    for ids in groups.values() {
        if ids.len() != 2 {
            continue;
        }
        let has = |id: ObjId, d: Direction| g.ports.values().any(|p| p.node == id && p.direction == d && !p.monitor);
        let (a, b) = (ids[0], ids[1]);
        let (input_side, output_side) = if has(a, Direction::In) && !has(b, Direction::In) && has(b, Direction::Out) {
            (a, b)
        } else if has(b, Direction::In) && !has(a, Direction::In) && has(a, Direction::Out) {
            (b, a)
        } else {
            continue;
        };
        merged_into.insert(output_side, input_side);
    }

    let mut units = Vec::new();
    for n in g.nodes.values() {
        if merged_into.contains_key(&n.id) {
            continue;
        }
        let partner = merged_into.iter().find(|(_, main)| **main == n.id).map(|(out, _)| *out);
        let mut inputs: Vec<&Port> = g.ports_of(n.id, Direction::In);
        let mut outputs: Vec<&Port> = match partner {
            Some(p) => g.ports_of(p, Direction::Out),
            None => g.ports_of(n.id, Direction::Out),
        };
        let all: Vec<&Port> = inputs.iter().chain(outputs.iter()).copied().collect();
        if all.is_empty() {
            continue;
        }
        let media = if all.iter().any(|p| p.media == Media::Audio) {
            Media::Audio
        } else if all.iter().any(|p| p.media == Media::Midi) {
            Media::Midi
        } else if all.iter().any(|p| p.media == Media::Video) {
            Media::Video
        } else {
            Media::Other
        };
        if media != Media::Audio && !include_other {
            continue;
        }
        if media == Media::Audio {
            inputs.retain(|p| p.media == Media::Audio);
            outputs.retain(|p| p.media == Media::Audio);
        }
        let kind = if media != Media::Audio {
            Kind::Other
        } else if partner.is_some() {
            Kind::Effect
        } else {
            n.kind()
        };
        let monitor_out = !outputs.is_empty() && outputs.iter().all(|p| p.monitor);
        let mut nodes = vec![n.id];
        nodes.extend(partner);
        let running = nodes.iter().any(|id| g.nodes.get(id).is_some_and(|m| m.state == NodeState::Running));
        units.push(Unit {
            key: String::new(),
            kind,
            main: n.id,
            nodes,
            title: n.title(),
            subtitle: subtitle(g, n, kind, media),
            inputs: inputs.iter().map(|p| p.id).collect(),
            outputs: outputs.iter().map(|p| p.id).collect(),
            monitor_out,
            is_default: g.is_default(n),
            running,
            media,
        });
    }
    units.sort_by_key(|u| (u.kind, u.title.to_lowercase(), u.main));
    assign_keys(g, &mut units);
    units
}

fn subtitle(g: &Graph, n: &Node, kind: Kind, media: Media) -> String {
    match kind {
        Kind::Player | Kind::Recorder => n
            .media_name
            .clone()
            .filter(|m| !m.trim().is_empty() && *m != n.title())
            .or_else(|| n.binary.clone())
            .unwrap_or_else(|| if kind == Kind::Player { "Playing".into() } else { "Recording".into() }),
        Kind::Output | Kind::Input => g
            .route_of(n)
            .map(|(_, r)| r.description.clone())
            .filter(|d| !d.is_empty())
            .or_else(|| n.nick.clone().filter(|nick| Some(nick) != n.description.as_ref()))
            .unwrap_or_else(|| kind.label().into()),
        Kind::Effect => "Effect".into(),
        Kind::Virtual => {
            if n.name.starts_with(crate::effects::NODE_PREFIX) {
                "Mix bus".into()
            } else if n.is_sink() {
                "Virtual output".into()
            } else {
                "Virtual input".into()
            }
        }
        Kind::Other => match media {
            Media::Midi => "MIDI".into(),
            Media::Video => "Video".into(),
            _ => n.media_class.clone(),
        },
    }
}

/// Devices and effects are known by node name; app streams by app and
/// kind, numbered when an app has several.
fn assign_keys(g: &Graph, units: &mut [Unit]) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<usize> = (0..units.len()).collect();
    order.sort_by_key(|&i| units[i].main);
    for i in order {
        let n = &g.nodes[&units[i].main];
        let base = if n.is_stream() && units[i].kind != Kind::Effect {
            let who = n.app.as_deref().unwrap_or(&n.name);
            format!("app:{}:{:?}", who, units[i].kind)
        } else {
            format!("node:{}", n.name)
        };
        let count = seen.entry(base.clone()).or_insert(0);
        *count += 1;
        units[i].key = if *count == 1 { base } else { format!("{base}#{count}") };
    }
}

/// The cords between `units`, bundling every port link between two cards.
pub fn cords(g: &Graph, units: &[Unit]) -> Vec<Cord> {
    let mut out_of: HashMap<ObjId, usize> = HashMap::new();
    let mut in_of: HashMap<ObjId, usize> = HashMap::new();
    for (i, u) in units.iter().enumerate() {
        out_of.extend(u.outputs.iter().map(|p| (*p, i)));
        in_of.extend(u.inputs.iter().map(|p| (*p, i)));
    }
    let mut cords: Vec<Cord> = Vec::new();
    for l in g.links.values() {
        let (Some(&from), Some(&to)) = (out_of.get(&l.out_port), in_of.get(&l.in_port)) else {
            continue;
        };
        let active = l.state == LinkState::Active;
        match cords.iter_mut().find(|c| c.from == from && c.to == to) {
            Some(c) => {
                c.links.push(l.id);
                c.active |= active;
            }
            None => cords.push(Cord { from, to, links: vec![l.id], active, monitor: units[from].monitor_out }),
        }
    }
    for c in &mut cords {
        c.active &= units[c.from].running;
    }
    cords
}

/// Port pairs that connect `from`'s outputs to `to`'s inputs, skipping
/// links that already exist.
pub fn plan_connect(g: &Graph, from: &Unit, to: &Unit) -> Vec<(ObjId, ObjId)> {
    let outs: Vec<&Port> = from.outputs.iter().filter_map(|id| g.ports.get(id)).collect();
    let ins: Vec<&Port> = to.inputs.iter().filter_map(|id| g.ports.get(id)).collect();
    plan_links(&outs, &ins)
        .into_iter()
        .filter(|(o, i)| g.link_between(*o, *i).is_none())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pwdump::parse_dump;

    fn fixture() -> Graph {
        let mut g = Graph::default();
        g.apply(parse_dump(include_str!("../tests/fixtures/pw-dump.json")).unwrap());
        g
    }

    #[test]
    fn effects_merge_into_one_card() {
        let g = fixture();
        let units = build(&g, false);
        let eq: Vec<&Unit> = units.iter().filter(|u| u.kind == Kind::Effect).collect();
        assert_eq!(eq.len(), 1, "{units:#?}");
        let eq = eq[0];
        assert_eq!(eq.nodes.len(), 2);
        assert_eq!(g.nodes[&eq.main].name, "m3sound.eq.test");
        assert_eq!(eq.inputs.len(), 2);
        assert!(eq.outputs.iter().all(|p| g.ports[p].node == eq.nodes[1]), "outputs come from the output side");
        assert!(!eq.monitor_out);
        assert!(units.iter().all(|u| g.nodes[&u.main].name != "m3sound.eq.test.out"));
        assert!(units.iter().any(|u| u.key == "node:test_speakers" && u.monitor_out));
        assert!(units.iter().any(|u| u.key == "app:Spotify:Player"));
        // Drivers without ports aren't cards.
        assert!(units.iter().all(|u| !u.title.contains("Driver")));
    }

    #[test]
    fn cords_bundle_links() {
        let g = fixture();
        let units = build(&g, false);
        let cords = cords(&g, &units);
        let idx = |key: &str| units.iter().position(|u| u.key == key).unwrap();
        let eq = units.iter().position(|u| u.kind == Kind::Effect).unwrap();
        let c = cords.iter().find(|c| c.from == eq && c.to == idx("node:test_headphones")).unwrap();
        assert_eq!(c.links.len(), 2);
        // Connecting what's connected plans nothing; a new pair plans both channels.
        assert!(plan_connect(&g, &units[eq], &units[idx("node:test_headphones")]).is_empty());
        assert_eq!(plan_connect(&g, &units[idx("app:Spotify:Player")], &units[eq]).len(), 2);
    }

    #[test]
    fn duplicate_apps_get_numbered_keys() {
        let mut g = fixture();
        let spotify = g.node_by_name("spotify").unwrap().clone();
        let port = g.ports.values().find(|p| p.node == spotify.id).unwrap().clone();
        g.nodes.insert(900, Node { id: 900, ..spotify });
        g.ports.insert(901, Port { id: 901, node: 900, ..port });
        let units = build(&g, false);
        assert!(units.iter().any(|u| u.key == "app:Spotify:Player"));
        assert!(units.iter().any(|u| u.key == "app:Spotify:Player#2" && u.main == 900));
    }
}
