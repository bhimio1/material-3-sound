//! `m3-sound --restore`: start saved effects and reconnect the
//! active scene, without a window, then exit. For running at login.

use crate::backend::{Backend, Command, Event, Status};
use crate::config::Config;
use crate::graph::Graph;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Wait for a graph that satisfies `ok`; the latest graph either way.
fn wait(b: &Backend, timeout: Duration, mut latest: Option<Arc<Graph>>, ok: impl Fn(&Graph) -> bool) -> (Option<Arc<Graph>>, bool) {
    let deadline = Instant::now() + timeout;
    if latest.as_deref().is_some_and(&ok) {
        return (latest, true);
    }
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match b.wait_event(left) {
            Some(Event::Graph(g)) => {
                let good = ok(&g);
                latest = Some(g);
                if good {
                    return (latest, true);
                }
            }
            Some(Event::Error(e)) => eprintln!("{e}"),
            Some(Event::Status(Status::Failed(e))) => eprintln!("PipeWire: {e}"),
            Some(_) => {}
            None => break,
        }
    }
    (latest, false)
}

pub fn run() -> i32 {
    let (cfg, err) = Config::load();
    if let Some(e) = err {
        eprintln!("{e}");
        return 1;
    }
    let b = Backend::pipewire(None);
    let (graph, ok) = wait(&b, Duration::from_secs(10), None, |g| !g.nodes.is_empty());
    let Some(mut graph) = graph.filter(|_| ok) else {
        eprintln!("Couldn't reach PipeWire.");
        return 1;
    };
    let mut started = Vec::new();
    for e in cfg.effects.iter().filter(|e| e.enabled) {
        if graph.node_by_name(&e.node_name()).is_some() {
            println!("{} is already running.", e.name);
        } else {
            b.send(Command::StartEffect(e.clone()));
            started.push(e.clone());
        }
    }
    let mut status = 0;
    // An effect is ready once its nodes have ports to connect to.
    let ready = |g: &Graph, e: &crate::effects::Effect| {
        std::iter::once(e.node_name()).chain(e.output_node_name()).all(|name| {
            g.node_by_name(&name).is_some_and(|n| g.ports.values().any(|p| p.node == n.id))
        })
    };
    if !started.is_empty() {
        let (g, _) = wait(&b, Duration::from_secs(8), Some(graph.clone()), |g| started.iter().all(|e| ready(g, e)));
        graph = g.unwrap_or(graph);
        for e in &started {
            if ready(&graph, e) {
                println!("Started {}.", e.name);
            } else {
                eprintln!("{} didn't start.", e.name);
                status = 1;
            }
        }
    }
    if let Some(scene) = cfg.active_scene.as_deref().and_then(|n| cfg.scene(n)) {
        // Keep reconciling while things settle (WirePlumber links new
        // nodes too), until the scene has held for a second.
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut held_since: Option<Instant> = None;
        let (mut connected, mut removed) = (HashSet::new(), HashSet::new());
        while Instant::now() < deadline {
            let plan = scene.reconcile(&graph);
            if plan.is_empty() {
                let since = *held_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_secs(1) {
                    break;
                }
            } else {
                held_since = None;
                let disconnect: Vec<_> = plan.disconnect.into_iter().filter(|l| removed.insert(*l)).collect();
                let connect: Vec<_> = plan.connect.into_iter().filter(|p| connected.insert(*p)).collect();
                if !disconnect.is_empty() {
                    b.send(Command::Disconnect(disconnect));
                }
                if !connect.is_empty() {
                    b.send(Command::Connect(connect));
                }
            }
            if let (Some(g), _) = wait(&b, Duration::from_millis(250), None, |_| true) {
                graph = g;
            }
        }
        let done = scene.reconcile(&graph).is_empty();
        if done {
            println!("Scene “{}” is connected.", scene.name);
        } else {
            println!("Scene “{}” is connected where it can be; some of its apps aren't running yet.", scene.name);
        }
        let _ = (connected, removed);
    }
    status
}
