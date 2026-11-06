//! Talking to PipeWire through its command-line tools, on background
//! threads: `pw-dump --monitor` streams the graph, and commands run
//! `pw-link`, `pw-cli` and `pw-metadata`. Equalizers run as their own
//! `pipewire -c` process. The demo backend simulates a desktop's graph.

use crate::effects::{Effect, EffectKind};
use crate::graph::{Graph, ObjId};
use crate::pwdump;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum VolumeTarget {
    Node(ObjId),
    /// A hardware route: set on the device so WirePlumber remembers it.
    Route { device: ObjId, index: i64, route_device: i64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Link output ports to input ports.
    Connect(Vec<(ObjId, ObjId)>),
    Disconnect(Vec<ObjId>),
    /// Linear channel volumes.
    SetVolume { target: VolumeTarget, channels: Vec<f32>, mute: bool },
    SetDefault { sink: bool, name: String },
    /// filter-chain controls on an equalizer's input node.
    SetControls { node: ObjId, controls: Vec<(String, f32)> },
    StartEffect(Effect),
    /// Stop an effect: every process running its configuration
    /// (equalizers), else its nodes (buses).
    StopEffect { id: String, nodes: Vec<ObjId> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Connecting,
    Connected,
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum Event {
    Graph(Arc<Graph>),
    Status(Status),
    Error(String),
}

pub struct Backend {
    tx: flume::Sender<Command>,
    rx: flume::Receiver<Event>,
    pub demo: bool,
    monitor: Arc<Mutex<Option<Child>>>,
}

impl Drop for Backend {
    fn drop(&mut self) {
        if let Ok(mut m) = self.monitor.lock() {
            if let Some(mut child) = m.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

type Repaint = Option<egui::Context>;

fn repaint(ctx: &Repaint) {
    if let Some(c) = ctx {
        c.request_repaint();
    }
}

impl Backend {
    /// The real thing. `ctx` is woken on every change.
    pub fn pipewire(ctx: Repaint) -> Self {
        let (tx, cmd_rx) = flume::unbounded();
        let (ev_tx, rx) = flume::unbounded();
        let monitor = Arc::new(Mutex::new(None));
        {
            let (ev_tx, ctx, monitor) = (ev_tx.clone(), ctx.clone(), monitor.clone());
            std::thread::Builder::new()
                .name("pw-monitor".into())
                .spawn(move || monitor_loop(ev_tx, ctx, monitor))
                .expect("spawn monitor thread");
        }
        std::thread::Builder::new()
            .name("pw-commands".into())
            .spawn(move || command_loop(cmd_rx, ev_tx, ctx))
            .expect("spawn command thread");
        Self { tx, rx, demo: false, monitor }
    }

    /// A simulated desktop, for trying the app without PipeWire.
    pub fn demo(ctx: Repaint) -> Self {
        let (tx, cmd_rx) = flume::unbounded();
        let (ev_tx, rx) = flume::unbounded();
        std::thread::Builder::new()
            .name("demo".into())
            .spawn(move || crate::demo::run(cmd_rx, ev_tx, ctx))
            .expect("spawn demo thread");
        Self { tx, rx, demo: true, monitor: Arc::new(Mutex::new(None)) }
    }

    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }

    pub fn events(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }

    pub fn wait_event(&self, timeout: Duration) -> Option<Event> {
        self.rx.recv_timeout(timeout).ok()
    }
}

fn monitor_loop(events: flume::Sender<Event>, ctx: Repaint, slot: Arc<Mutex<Option<Child>>>) {
    let mut delay = Duration::from_millis(500);
    loop {
        let _ = events.send(Event::Status(Status::Connecting));
        repaint(&ctx);
        let spawned = Process::new("pw-dump")
            .args(["--monitor", "--no-colors"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                let msg = if e.kind() == std::io::ErrorKind::NotFound {
                    "pw-dump isn't installed. It comes with PipeWire (the pipewire package).".to_string()
                } else {
                    format!("pw-dump: {e}")
                };
                if events.send(Event::Status(Status::Failed(msg))).is_err() {
                    return;
                }
                repaint(&ctx);
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
        };
        let stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        if let Ok(mut s) = slot.lock() {
            *s = Some(child);
        }
        let mut graph = Graph::default();
        let mut connected = false;
        if let Some(out) = stdout {
            let stream = serde_json::Deserializer::from_reader(BufReader::new(out)).into_iter::<Vec<serde_json::Value>>();
            for batch in stream {
                let Ok(batch) = batch else { break };
                graph.apply(pwdump::parse_batch(&batch));
                if !connected {
                    connected = true;
                    delay = Duration::from_millis(500);
                    let _ = events.send(Event::Status(Status::Connected));
                }
                if events.send(Event::Graph(Arc::new(graph.clone()))).is_err() {
                    return;
                }
                repaint(&ctx);
            }
        }
        let mut err = String::new();
        if let Some(s) = stderr.as_mut() {
            let _ = s.take(4096).read_to_string(&mut err);
        }
        if let Ok(mut s) = slot.lock() {
            match s.take() {
                Some(mut c) => {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                // Taken by Backend::drop: the app is closing.
                None => return,
            }
        }
        let reason = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("PipeWire isn't running.").trim().to_string();
        let _ = events.send(Event::Graph(Arc::new(Graph::default())));
        if events.send(Event::Status(Status::Failed(reason))).is_err() {
            return;
        }
        repaint(&ctx);
        std::thread::sleep(delay);
        delay = (delay * 2).min(Duration::from_secs(5));
    }
}

/// Run a PipeWire tool. pw-cli exits 0 even when a command fails and
/// prints `Error: …` instead, so that counts as a failure too.
fn tool(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Process::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let error_line = stdout.lines().chain(stderr.lines()).find(|l| l.trim_start().starts_with("Error:"));
    if out.status.success() && error_line.is_none() {
        return Ok(stdout);
    }
    let msg = error_line
        .map(str::to_string)
        .or_else(|| stderr.lines().find(|l| !l.trim().is_empty()).map(str::to_string))
        .unwrap_or_else(|| format!("exited with {}", out.status));
    Err(format!("{program}: {}", msg.trim()))
}

/// Keep only the last volume per target and merge control changes, so a
/// dragged slider doesn't queue up hundreds of commands.
pub fn coalesce(batch: Vec<Command>) -> Vec<Command> {
    let mut out: Vec<Command> = Vec::new();
    for c in batch {
        match &c {
            Command::SetVolume { target, .. } => {
                out.retain(|o| !matches!(o, Command::SetVolume { target: t, .. } if t == target));
                out.push(c);
            }
            Command::SetControls { node, controls } => {
                if let Some(Command::SetControls { controls: existing, .. }) =
                    out.iter_mut().find(|o| matches!(o, Command::SetControls { node: n, .. } if n == node))
                {
                    for (k, v) in controls {
                        match existing.iter_mut().find(|(ek, _)| ek == k) {
                            Some(e) => e.1 = *v,
                            None => existing.push((k.clone(), *v)),
                        }
                    }
                } else {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn command_loop(rx: flume::Receiver<Command>, events: flume::Sender<Event>, ctx: Repaint) {
    let mut children: Vec<Child> = Vec::new();
    loop {
        let first = match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(c) => c,
            Err(flume::RecvTimeoutError::Timeout) => {
                children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
                continue;
            }
            Err(flume::RecvTimeoutError::Disconnected) => return,
        };
        let mut batch = vec![first];
        batch.extend(rx.try_iter());
        for c in coalesce(batch) {
            if let Err(e) = run(c, &mut children) {
                let _ = events.send(Event::Error(e));
                repaint(&ctx);
            }
        }
        children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
    }
}

fn floats(v: &[f32]) -> String {
    v.iter().map(|x| format!("{:.6}", x)).collect::<Vec<_>>().join(", ")
}

fn run(c: Command, children: &mut Vec<Child>) -> Result<(), String> {
    match c {
        Command::Connect(pairs) => {
            let mut errors = Vec::new();
            for (o, i) in pairs {
                if let Err(e) = tool("pw-link", &[&o.to_string(), &i.to_string()]) {
                    if !e.contains("File exists") {
                        errors.push(e);
                    }
                }
            }
            errors.into_iter().next().map_or(Ok(()), |e| Err(format!("Couldn't connect: {e}")))
        }
        Command::Disconnect(links) => {
            let mut errors = Vec::new();
            for l in links {
                if let Err(e) = tool("pw-link", &["-d", &l.to_string()]) {
                    if !e.contains("No such file") {
                        errors.push(e);
                    }
                }
            }
            errors.into_iter().next().map_or(Ok(()), |e| Err(format!("Couldn't disconnect: {e}")))
        }
        Command::SetVolume { target, channels, mute } => {
            let props = format!("{{ \"mute\": {mute}, \"channelVolumes\": [ {} ] }}", floats(&channels));
            match target {
                VolumeTarget::Node(id) => tool("pw-cli", &["set-param", &id.to_string(), "Props", &props]),
                VolumeTarget::Route { device, index, route_device } => {
                    let route = format!("{{ \"index\": {index}, \"device\": {route_device}, \"props\": {props}, \"save\": true }}");
                    tool("pw-cli", &["set-param", &device.to_string(), "Route", &route])
                }
            }
            .map(|_| ())
            .map_err(|e| format!("Couldn't change the volume: {e}"))
        }
        Command::SetDefault { sink, name } => {
            let key = if sink { "default.configured.audio.sink" } else { "default.configured.audio.source" };
            let value = serde_json::json!({ "name": name }).to_string();
            tool("pw-metadata", &["0", key, &value, "Spa:String:JSON"])
                .map(|_| ())
                .map_err(|e| format!("Couldn't change the default device: {e}"))
        }
        Command::SetControls { node, controls } => {
            let list = controls
                .iter()
                .map(|(k, v)| format!("{}, {:.4}", serde_json::Value::String(k.clone()), v))
                .collect::<Vec<_>>()
                .join(", ");
            tool("pw-cli", &["set-param", &node.to_string(), "Props", &format!("{{ \"params\": [ {list} ] }}")])
                .map(|_| ())
                .map_err(|e| format!("Couldn't update the equalizer: {e}"))
        }
        Command::StartEffect(effect) => start_effect(&effect, children),
        Command::StopEffect { id, nodes } => {
            let pids = effect_pids(&id);
            if pids.is_empty() {
                for n in nodes {
                    tool("pw-cli", &["destroy", &n.to_string()])?;
                }
            }
            for pid in pids {
                tool("kill", &["-TERM", &pid.to_string()])?;
            }
            Ok(())
        }
    }
}

fn start_effect(effect: &Effect, children: &mut Vec<Child>) -> Result<(), String> {
    match &effect.kind {
        EffectKind::Bus(_) => {
            let props = effect.bus_props().unwrap_or_default();
            tool("pw-cli", &["create-node", "adapter", &props]).map(|_| ()).map_err(|e| format!("Couldn't create {}: {e}", effect.name))
        }
        EffectKind::Equalizer(_) => {
            // Already running (its nodes may just not be listed yet).
            if !effect_pids(&effect.id).is_empty() {
                return Ok(());
            }
            let dir = crate::config::effects_dir();
            let conf = write_conf(&dir, effect)?;
            let log_path = dir.join(format!("{}.log", effect.id));
            let log = std::fs::File::create(&log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
            use std::os::unix::process::CommandExt;
            let mut child = Process::new("pipewire")
                .arg("-c")
                .arg(&conf)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(log)
                .process_group(0)
                .spawn()
                .map_err(|e| format!("Couldn't start {}: pipewire: {e}", effect.name))?;
            std::thread::sleep(Duration::from_millis(300));
            if let Ok(Some(status)) = child.try_wait() {
                let text = std::fs::read_to_string(&log_path).unwrap_or_default();
                let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
                return Err(format!("{} stopped right away ({status}). {last}", effect.name));
            }
            children.push(child);
            Ok(())
        }
    }
}

/// Write an equalizer's configuration; returns its path.
pub fn write_conf(dir: &Path, effect: &Effect) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.conf", effect.id));
    let text = effect.filter_chain_conf().ok_or("not an equalizer")?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Processes running an effect's generated configuration. Only these
/// are ever stopped, and an effect that has one isn't started twice.
pub fn effect_pids(id: &str) -> Vec<u32> {
    let needle = format!("m3-sound/effects/{id}.conf");
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            std::fs::read(format!("/proc/{pid}/cmdline"))
                .map(|c| String::from_utf8_lossy(&c).contains(&needle))
                .unwrap_or(false)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes_and_controls_coalesce() {
        let v = |x: f32| Command::SetVolume { target: VolumeTarget::Node(5), channels: vec![x], mute: false };
        let other = Command::SetVolume { target: VolumeTarget::Node(6), channels: vec![0.1], mute: false };
        let out = coalesce(vec![v(0.1), Command::Disconnect(vec![1]), v(0.2), other.clone(), v(0.3)]);
        assert_eq!(out, vec![Command::Disconnect(vec![1]), other, v(0.3)]);
        let c = |k: &str, x: f32| Command::SetControls { node: 9, controls: vec![(k.into(), x)] };
        let out = coalesce(vec![c("a:Gain", 1.0), c("b:Gain", 2.0), c("a:Gain", 3.0)]);
        assert_eq!(out, vec![Command::SetControls { node: 9, controls: vec![("a:Gain".into(), 3.0), ("b:Gain".into(), 2.0)] }]);
    }

    #[test]
    fn pw_cli_errors_are_failures() {
        assert!(tool("sh", &["-c", "echo 'Error: \"set-param: unknown global\"'"]).is_err());
        assert_eq!(tool("sh", &["-c", "echo fine"]).unwrap().trim(), "fine");
        assert!(tool("sh", &["-c", "echo oops >&2; exit 3"]).unwrap_err().contains("oops"));
        assert!(tool("definitely-not-a-program-m3s", &[]).is_err());
    }

    /// The latest graph once it satisfies `ok` (checking the one we have first).
    fn wait_for(b: &Backend, latest: &mut Arc<Graph>, what: &str, ok: impl Fn(&Graph) -> bool) -> Arc<Graph> {
        if ok(latest) {
            return latest.clone();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while std::time::Instant::now() < deadline {
            match b.wait_event(Duration::from_millis(200)) {
                Some(Event::Graph(g)) => {
                    *latest = g.clone();
                    if ok(&g) {
                        return g;
                    }
                }
                Some(Event::Error(e)) => panic!("{what}: {e}"),
                _ => {}
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Against a running PipeWire with the `test_speakers` null sink and a
    /// `spotify` player: `M3S_PIPEWIRE_TEST=1 scripts/pipewire-test-session.sh
    /// cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn real_pipewire_round_trip() {
        use crate::graph::Direction;
        if std::env::var_os("M3S_PIPEWIRE_TEST").is_none() {
            return;
        }
        let b = Backend::pipewire(None);
        let mut g = Arc::new(Graph::default());
        let found = wait_for(&b, &mut g, "the graph", |g| {
            let ports = |name: &str| g.node_by_name(name).is_some_and(|n| g.ports.values().filter(|p| p.node == n.id).count() >= 2);
            ports("test_speakers") && ports("spotify")
        });
        let (player, speakers) = (found.node_by_name("spotify").unwrap().id, found.node_by_name("test_speakers").unwrap().id);
        let between = |g: &Graph| -> Vec<ObjId> { g.links.values().filter(|l| l.out_node == player && l.in_node == speakers).map(|l| l.id).collect() };

        // Start unplugged (WirePlumber may have linked the player already).
        let existing = between(&found);
        if !existing.is_empty() {
            b.send(Command::Disconnect(existing));
        }
        let g0 = wait_for(&b, &mut g, "the disconnect", |g| between(g).is_empty());
        let outs = g0.ports_of(player, Direction::Out);
        let ins = g0.ports_of(speakers, Direction::In);
        b.send(Command::Connect(crate::graph::plan_links(&outs, &ins)));
        wait_for(&b, &mut g, "the cord", |g| between(g).len() == 2);

        b.send(Command::SetVolume { target: VolumeTarget::Node(player), channels: vec![0.2, 0.1], mute: true });
        wait_for(&b, &mut g, "the volume", |g| {
            g.nodes[&player].volume.as_ref().is_some_and(|v| v.mute && (v.channels[1] - 0.1).abs() < 1e-3)
        });
        b.send(Command::SetVolume { target: VolumeTarget::Node(player), channels: vec![1.0, 1.0], mute: false });
        b.send(Command::SetDefault { sink: true, name: "test_headphones".into() });
        wait_for(&b, &mut g, "the default", |g| g.defaults.sink.as_deref() == Some("test_headphones"));
        b.send(Command::SetDefault { sink: true, name: "test_speakers".into() });
        wait_for(&b, &mut g, "the default back", |g| g.defaults.sink.as_deref() == Some("test_speakers"));

        let mut eq = Effect::new_equalizer("Test EQ");
        eq.id = format!("itest{}", std::process::id());
        let name = eq.node_name();
        b.send(Command::StartEffect(eq.clone()));
        let started = wait_for(&b, &mut g, "the equalizer", |g| g.node_by_name(&name).is_some() && g.node_by_name(&format!("{name}.out")).is_some());
        let node = started.node_by_name(&name).unwrap().clone();
        b.send(Command::SetControls { node: node.id, controls: vec![("band1:Gain".into(), 4.5)] });
        wait_for(&b, &mut g, "the band gain", |g| g.nodes.get(&node.id).is_some_and(|n| n.controls.contains(&("band1:Gain".into(), 4.5))));
        assert!(started.pid_of(&node).is_some());
        assert_eq!(effect_pids(&eq.id).len(), 1);
        // Asking again doesn't start a second copy.
        b.send(Command::StartEffect(eq.clone()));
        b.send(Command::StopEffect { id: eq.id.clone(), nodes: vec![] });
        wait_for(&b, &mut g, "the equalizer to stop", |g| g.node_by_name(&name).is_none());
        assert!(effect_pids(&eq.id).is_empty());

        let mut bus = Effect::new_bus("Test bus");
        bus.id = format!("itest{}", std::process::id());
        let name = bus.node_name();
        b.send(Command::StartEffect(bus.clone()));
        let made = wait_for(&b, &mut g, "the bus", |g| g.node_by_name(&name).is_some());
        let id = made.node_by_name(&name).unwrap().id;
        b.send(Command::StopEffect { id: bus.id.clone(), nodes: vec![id] });
        wait_for(&b, &mut g, "the bus to go", |g| g.node_by_name(&name).is_none());
    }

    #[test]
    fn effect_processes_are_found_by_their_configuration() {
        let id = format!("unit{}", std::process::id());
        assert!(effect_pids(&id).is_empty());
        let conf = format!("/tmp/m3-sound/effects/{id}.conf");
        let mut child = Process::new("sh").args(["-c", "sleep 5", "sh", &conf]).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(effect_pids(&id), vec![child.id()]);
        assert!(effect_pids(&format!("{id}x")).is_empty());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(effect_pids(&id).is_empty());
    }
}
