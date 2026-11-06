//! Scenes: named wirings ("Gaming", "Recording") saved by node and port
//! names, so they survive restarts. The active scene is kept connected:
//! when an app in it starts, its cords come back, and an exclusive scene
//! also removes other cords from its sources (like WirePlumber's
//! automatic link to the default device).

use crate::graph::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SceneLink {
    pub out_node: String,
    pub out_port: String,
    pub in_node: String,
    pub in_port: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    pub links: Vec<SceneLink>,
    /// Remove other cords from this scene's sources.
    #[serde(default = "yes")]
    pub exclusive: bool,
}

fn yes() -> bool {
    true
}

/// What to change to make the graph match a scene.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub connect: Vec<(ObjId, ObjId)>,
    pub disconnect: Vec<ObjId>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.connect.is_empty() && self.disconnect.is_empty()
    }
}

/// A link described by names, if both ends are audio ports of known nodes.
pub fn describe(g: &Graph, out_port: ObjId, in_port: ObjId) -> Option<SceneLink> {
    let (op, ip) = (g.ports.get(&out_port)?, g.ports.get(&in_port)?);
    if op.media != Media::Audio || ip.media != Media::Audio {
        return None;
    }
    Some(SceneLink {
        out_node: g.nodes.get(&op.node)?.name.clone(),
        out_port: op.name.clone(),
        in_node: g.nodes.get(&ip.node)?.name.clone(),
        in_port: ip.name.clone(),
    })
}

impl Scene {
    /// Every audio link in the graph right now.
    pub fn capture(name: &str, g: &Graph, exclusive: bool) -> Self {
        let mut seen = HashSet::new();
        let links = g
            .links
            .values()
            .filter_map(|l| describe(g, l.out_port, l.in_port))
            .filter(|s| seen.insert(s.clone()))
            .collect();
        Self { name: name.into(), links, exclusive }
    }

    /// Remember links the user just made. A source new to the scene
    /// brings its current cords along, so an exclusive scene doesn't
    /// remove cords the user never touched.
    pub fn add(&mut self, g: &Graph, pairs: &[(ObjId, ObjId)]) {
        let sources: HashSet<String> = self.links.iter().map(|l| l.out_node.clone()).collect();
        let mut new = Vec::new();
        for (o, i) in pairs {
            let Some(s) = describe(g, *o, *i) else { continue };
            if !sources.contains(&s.out_node) {
                new.extend(
                    g.links
                        .values()
                        .filter(|l| g.nodes.get(&l.out_node).is_some_and(|n| n.name == s.out_node))
                        .filter_map(|l| describe(g, l.out_port, l.in_port)),
                );
            }
            new.push(s);
        }
        for s in new {
            if !self.links.contains(&s) {
                self.links.push(s);
            }
        }
    }

    /// Forget links the user just removed.
    pub fn remove(&mut self, g: &Graph, link_ids: &[ObjId]) {
        let gone: Vec<SceneLink> = link_ids
            .iter()
            .filter_map(|id| g.links.get(id))
            .filter_map(|l| describe(g, l.out_port, l.in_port))
            .collect();
        self.links.retain(|s| !gone.contains(s));
    }

    /// Node pairs for display: "Spotify → Headphones", with link counts.
    pub fn pairs(&self) -> Vec<(String, String, usize)> {
        let mut v: Vec<(String, String, usize)> = Vec::new();
        for l in &self.links {
            match v.iter_mut().find(|(o, i, _)| *o == l.out_node && *i == l.in_node) {
                Some(e) => e.2 += 1,
                None => v.push((l.out_node.clone(), l.in_node.clone(), 1)),
            }
        }
        v
    }

    /// Drop every link between two nodes (a cord in the list).
    pub fn remove_pair(&mut self, out_node: &str, in_node: &str) {
        self.links.retain(|l| !(l.out_node == out_node && l.in_node == in_node));
    }

    /// What to connect and disconnect so the graph matches this scene.
    /// Nodes that aren't running are skipped until they appear.
    pub fn reconcile(&self, g: &Graph) -> Plan {
        let mut plan = Plan::default();
        let ports_named = |node: &str, port: &str, dir: Direction| -> Vec<ObjId> {
            g.ports
                .values()
                .filter(|p| p.direction == dir && p.name == port)
                .filter(|p| g.nodes.get(&p.node).is_some_and(|n| n.name == node))
                .map(|p| p.id)
                .collect()
        };
        let mut wanted: HashSet<(ObjId, ObjId)> = HashSet::new();
        for l in &self.links {
            let outs = ports_named(&l.out_node, &l.out_port, Direction::Out);
            let ins = ports_named(&l.in_node, &l.in_port, Direction::In);
            for o in &outs {
                for i in &ins {
                    wanted.insert((*o, *i));
                    if g.link_between(*o, *i).is_none() && !plan.connect.contains(&(*o, *i)) {
                        plan.connect.push((*o, *i));
                    }
                }
            }
        }
        if self.exclusive {
            let sources: HashSet<&str> = self.links.iter().map(|l| l.out_node.as_str()).collect();
            for link in g.links.values() {
                let Some(node) = g.nodes.get(&link.out_node) else { continue };
                let audio = g.ports.get(&link.out_port).is_some_and(|p| p.media == Media::Audio);
                if audio && sources.contains(node.name.as_str()) && !wanted.contains(&(link.out_port, link.in_port)) {
                    plan.disconnect.push(link.id);
                }
            }
        }
        plan.connect.sort_unstable();
        plan.disconnect.sort_unstable();
        plan
    }
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

    fn port(g: &Graph, node: &str, port: &str) -> ObjId {
        let n = g.node_by_name(node).unwrap().id;
        g.ports.values().find(|p| p.node == n && p.name == port).unwrap().id
    }

    #[test]
    fn a_captured_scene_is_already_satisfied() {
        let g = fixture();
        let s = Scene::capture("Now", &g, true);
        assert!(!s.links.is_empty());
        assert!(s.reconcile(&g).is_empty(), "{:?}", s.reconcile(&g));
        assert!(s.pairs().iter().any(|(o, i, n)| o == "spotify" && i == "test_headphones" && *n == 2));
    }

    #[test]
    fn reconnects_and_removes_strays() {
        let g = fixture();
        let mut s = Scene { name: "Speakers".into(), links: vec![], exclusive: true };
        let pairs = [
            (port(&g, "spotify", "output_FL"), port(&g, "test_speakers", "playback_FL")),
            (port(&g, "spotify", "output_FR"), port(&g, "test_speakers", "playback_FR")),
        ];
        s.links = pairs.iter().filter_map(|(o, i)| describe(&g, *o, *i)).collect();
        let plan = s.reconcile(&g);
        let mut want = pairs.to_vec();
        want.sort_unstable();
        assert_eq!(plan.connect, want);
        // Spotify's existing cord to the headphones goes: the scene is exclusive.
        let spotify = g.node_by_name("spotify").unwrap().id;
        let strays: Vec<ObjId> = g.links.values().filter(|l| l.out_node == spotify).map(|l| l.id).collect();
        assert!(!strays.is_empty());
        assert_eq!(plan.disconnect, strays);
        s.exclusive = false;
        assert!(s.reconcile(&g).disconnect.is_empty());
    }

    #[test]
    fn a_new_source_keeps_its_cords() {
        let g = fixture();
        let mut s = Scene { name: "Live".into(), links: vec![], exclusive: true };
        let pairs = [
            (port(&g, "spotify", "output_FL"), port(&g, "test_speakers", "playback_FL")),
            (port(&g, "spotify", "output_FR"), port(&g, "test_speakers", "playback_FR")),
        ];
        s.add(&g, &pairs);
        s.add(&g, &pairs);
        // The new cords, plus Spotify's two existing ones to the headphones.
        assert_eq!(s.links.len(), 4, "{:?}", s.links);
        let plan = s.reconcile(&g);
        assert!(plan.disconnect.is_empty(), "nothing the user had goes");
        assert_eq!(plan.connect.len(), 2);
    }

    #[test]
    fn removing_links_forgets_them() {
        let g = fixture();
        let mut s = Scene::capture("Now", &g, false);
        let spotify = g.node_by_name("spotify").unwrap().id;
        let ids: Vec<ObjId> = g.links.values().filter(|l| l.out_node == spotify).map(|l| l.id).collect();
        let before = s.links.len();
        s.remove(&g, &ids);
        assert_eq!(s.links.len(), before - ids.len());
        s.remove_pair("m3sound.eq.test.out", "test_headphones");
        assert!(s.pairs().iter().all(|(o, _, _)| o != "m3sound.eq.test.out"));
    }

    #[test]
    fn missing_nodes_wait() {
        let g = fixture();
        let s = Scene {
            name: "Later".into(),
            links: vec![SceneLink {
                out_node: "not-running".into(),
                out_port: "output_FL".into(),
                in_node: "test_speakers".into(),
                in_port: "playback_FL".into(),
            }],
            exclusive: true,
        };
        assert!(s.reconcile(&g).is_empty());
    }
}
