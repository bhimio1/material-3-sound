//! Reading `pw-dump` output: the JSON array it prints at start, and the
//! arrays `pw-dump --monitor` prints after every change (a removed object
//! comes as `{"id": 42, "info": null}`).

use crate::graph::*;
use serde_json::{Map, Value};

/// Every object in one `pw-dump` array.
pub fn parse_batch(batch: &[Value]) -> Vec<Update> {
    batch.iter().filter_map(parse_object).collect()
}

/// Parse a whole `pw-dump` output (one or more arrays).
#[cfg(test)]
pub fn parse_dump(text: &str) -> Result<Vec<Update>, String> {
    let mut out = Vec::new();
    for batch in serde_json::Deserializer::from_str(text).into_iter::<Vec<Value>>() {
        out.extend(parse_batch(&batch.map_err(|e| e.to_string())?));
    }
    Ok(out)
}

pub fn parse_object(o: &Value) -> Option<Update> {
    let id = o.get("id")?.as_u64()? as ObjId;
    let Some(ty) = o.get("type").and_then(Value::as_str) else {
        return Some(Update::Remove(id));
    };
    let info = o.get("info").and_then(Value::as_object);
    let empty = Map::new();
    let props = info
        .and_then(|i| i.get("props"))
        .or_else(|| o.get("props"))
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let obj = match ty.rsplit(':').next().unwrap_or("") {
        "Node" => Object::Node(Box::new(node(id, info?, props))),
        "Port" => Object::Port(port(id, info?, props)?),
        "Link" => Object::Link(link(id, info?)?),
        "Device" => Object::Device(device(id, info?, props)),
        "Client" => Object::Client(Client {
            id,
            pid: num(props, "application.process.id").or_else(|| num(props, "pipewire.sec.pid")).map(|p| p as u32),
            app: string(props, "application.name"),
        }),
        "Metadata" if string(props, "metadata.name").as_deref() == Some("default") => {
            Object::Defaults(defaults(id, o.get("metadata")))
        }
        "Core" => Object::Server(Server {
            version: info.and_then(|i| i.get("version")).and_then(Value::as_str).unwrap_or_default().to_string(),
            rate: num(props, "default.clock.rate").map(|v| v as u32),
            quantum: num(props, "default.clock.quantum").map(|v| v as u32),
        }),
        _ => Object::Other(id),
    };
    Some(Update::Upsert(obj))
}

/// A property as text (numbers and booleans too: pw-dump types them).
fn string(props: &Map<String, Value>, key: &str) -> Option<String> {
    match props.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn num(props: &Map<String, Value>, key: &str) -> Option<i64> {
    match props.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn flag(props: &Map<String, Value>, key: &str) -> bool {
    match props.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true" || s == "1",
        Some(Value::Number(n)) => n.as_i64() == Some(1),
        _ => false,
    }
}

fn node(id: ObjId, info: &Map<String, Value>, props: &Map<String, Value>) -> Node {
    let state = match info.get("state").and_then(Value::as_str) {
        Some("running") => NodeState::Running,
        Some("idle") => NodeState::Idle,
        Some("creating") => NodeState::Creating,
        Some("error") => NodeState::Error,
        _ => NodeState::Suspended,
    };
    let params = info.get("params").and_then(Value::as_object);
    let props_params = params.and_then(|p| p.get("Props")).and_then(Value::as_array);
    let mut volume = None;
    let mut controls = Vec::new();
    for entry in props_params.into_iter().flatten().filter_map(Value::as_object) {
        if volume.is_none() {
            volume = parse_volume(entry);
        }
        if !entry.contains_key("channelVolumes") {
            controls.extend(filter_controls(entry));
        }
    }
    Node {
        id,
        serial: num(props, "object.serial").unwrap_or(id as i64) as u64,
        name: string(props, "node.name").unwrap_or_else(|| format!("node-{id}")),
        description: string(props, "node.description"),
        nick: string(props, "node.nick"),
        app: string(props, "application.name"),
        binary: string(props, "application.process.binary"),
        icon_name: string(props, "application.icon-name").or_else(|| string(props, "device.icon-name")),
        media_name: string(props, "media.name"),
        media_class: string(props, "media.class").unwrap_or_default(),
        role: string(props, "media.role"),
        state,
        volume,
        device_id: num(props, "device.id").map(|v| v as ObjId),
        profile_device: num(props, "card.profile.device"),
        link_group: string(props, "node.link-group"),
        client_id: num(props, "client.id").map(|v| v as ObjId),
        factory: string(props, "factory.name"),
        api: string(props, "device.api"),
        virtual_: flag(props, "node.virtual") || string(props, "factory.name").as_deref() == Some("support.null-audio-sink"),
        controls,
    }
}

fn parse_volume(entry: &Map<String, Value>) -> Option<Volume> {
    let channels: Vec<f32> = entry
        .get("channelVolumes")?
        .as_array()?
        .iter()
        .filter_map(Value::as_f64)
        .map(|v| v as f32)
        .collect();
    if channels.is_empty() {
        return None;
    }
    let map = entry
        .get("channelMap")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    Some(Volume { channels, mute: entry.get("mute").and_then(Value::as_bool).unwrap_or(false), map })
}

/// filter-chain controls: `"params": ["band1:Freq", 100.0, …]`.
fn filter_controls(entry: &Map<String, Value>) -> Vec<(String, f32)> {
    let Some(list) = entry.get("params").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.chunks(2)
        .filter_map(|kv| {
            let key = kv.first()?.as_str()?;
            let value = kv.get(1)?.as_f64()?;
            key.contains(':').then(|| (key.to_string(), value as f32))
        })
        .collect()
}

fn port(id: ObjId, info: &Map<String, Value>, props: &Map<String, Value>) -> Option<Port> {
    let direction = match info.get("direction").and_then(Value::as_str)? {
        "input" => Direction::In,
        _ => Direction::Out,
    };
    let dsp = string(props, "format.dsp").unwrap_or_default();
    let media = if dsp.contains("audio") {
        Media::Audio
    } else if dsp.contains("midi") || dsp.contains("UMP") {
        Media::Midi
    } else if dsp.contains("video") {
        Media::Video
    } else {
        Media::Other
    };
    Some(Port {
        id,
        node: num(props, "node.id")? as ObjId,
        direction,
        name: string(props, "port.name").unwrap_or_else(|| format!("port-{id}")),
        channel: string(props, "audio.channel").unwrap_or_default(),
        media,
        monitor: flag(props, "port.monitor"),
    })
}

fn link(id: ObjId, info: &Map<String, Value>) -> Option<Link> {
    let get = |k: &str| info.get(k).and_then(Value::as_u64).map(|v| v as ObjId);
    let state = match info.get("state").and_then(Value::as_str) {
        Some("active") => LinkState::Active,
        Some("paused") => LinkState::Paused,
        Some("error") => LinkState::Error,
        _ => LinkState::Init,
    };
    Some(Link {
        id,
        out_node: get("output-node-id")?,
        out_port: get("output-port-id")?,
        in_node: get("input-node-id")?,
        in_port: get("input-port-id")?,
        state,
    })
}

fn device(id: ObjId, info: &Map<String, Value>, props: &Map<String, Value>) -> Device {
    let routes = info
        .get("params")
        .and_then(|p| p.get("Route"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|r| {
            Some(Route {
                index: r.get("index")?.as_i64()?,
                device: r.get("device")?.as_i64()?,
                direction: if r.get("direction").and_then(Value::as_str) == Some("Input") {
                    Direction::In
                } else {
                    Direction::Out
                },
                description: r.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
                volume: r.get("props").and_then(Value::as_object).and_then(parse_volume),
            })
        })
        .collect();
    Device {
        id,
        name: string(props, "device.name").unwrap_or_default(),
        description: string(props, "device.description").or_else(|| string(props, "device.nick")).unwrap_or_default(),
        routes,
    }
}

fn defaults(id: ObjId, metadata: Option<&Value>) -> Defaults {
    let mut d = Defaults { metadata_id: Some(id), ..Default::default() };
    for entry in metadata.and_then(Value::as_array).into_iter().flatten() {
        if entry.get("subject").and_then(Value::as_u64) != Some(0) {
            continue;
        }
        let Some(key) = entry.get("key").and_then(Value::as_str) else { continue };
        let name = entry.get("value").and_then(metadata_name);
        match key {
            "default.audio.sink" => d.sink = name,
            "default.audio.source" => d.source = name,
            "default.configured.audio.sink" => d.configured_sink = name,
            "default.configured.audio.source" => d.configured_source = name,
            _ => {}
        }
    }
    d
}

/// `{"name": "alsa_output…"}`, as an object or as JSON text.
fn metadata_name(v: &Value) -> Option<String> {
    match v {
        Value::Object(m) => m.get("name").and_then(Value::as_str).map(str::to_string),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Object(m)) => m.get("name").and_then(Value::as_str).map(str::to_string),
            _ => Some(s.clone()),
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Graph {
        let text = include_str!("../tests/fixtures/pw-dump.json");
        let mut g = Graph::default();
        g.apply(parse_dump(text).unwrap());
        g
    }

    #[test]
    fn reads_a_real_dump() {
        let g = fixture();
        let speakers = g.node_by_name("test_speakers").unwrap();
        assert_eq!(speakers.title(), "Speakers");
        assert_eq!(speakers.media_class, "Audio/Sink");
        assert!(speakers.virtual_, "null sinks are virtual");
        assert_eq!(speakers.volume.as_ref().unwrap().channels, vec![1.0, 1.0]);
        let ins = g.ports_of(speakers.id, Direction::In);
        assert_eq!(ins.iter().map(|p| p.channel.as_str()).collect::<Vec<_>>(), ["FL", "FR"]);
        let outs = g.ports_of(speakers.id, Direction::Out);
        assert!(outs.iter().all(|p| p.monitor), "a sink's outputs are monitors");

        let player = g.node_by_name("spotify").unwrap();
        assert_eq!(player.kind(), Kind::Player);
        assert_eq!(player.title(), "Spotify");
        assert_eq!(player.media_name.as_deref(), Some("Lo-fi beats"));
        let v = player.volume.as_ref().unwrap();
        assert_eq!(v.channels, vec![0.5, 0.25]);
        assert_eq!(v.map, vec!["FL", "FR"]);
        assert!(g.links.values().any(|l| l.out_node == player.id && l.state == LinkState::Active));

        assert_eq!(g.defaults.sink.as_deref(), Some("test_headphones"));
        assert_eq!(g.defaults.configured_sink.as_deref(), Some("test_headphones"));
        assert!(g.is_default(g.node_by_name("test_headphones").unwrap()));
        assert_eq!(g.server.as_ref().unwrap().rate, Some(48000));

        // The equalizer's input side carries its controls; the output side
        // is a stream in the same link group.
        let eq = g.node_by_name("m3sound.eq.test").unwrap();
        assert_eq!(eq.kind(), Kind::Effect);
        assert!(eq.controls.iter().any(|(k, v)| k == "band2:Freq" && *v == 1000.0));
        assert!(eq.controls.iter().all(|(k, _)| k.contains(':')));
        let eq_out = g.node_by_name("m3sound.eq.test.out").unwrap();
        assert_eq!(eq_out.link_group, eq.link_group);
        assert!(g.pid_of(eq).is_some());
    }

    #[test]
    fn removals_and_routes() {
        let mut g = fixture();
        let id = g.node_by_name("spotify").unwrap().id;
        g.apply(parse_dump(&format!("[{{\"id\": {id}, \"info\": null}}]")).unwrap());
        assert!(g.node_by_name("spotify").is_none());

        let dev = r#"[{"id": 70, "type": "PipeWire:Interface:Device", "info": {
            "props": {"device.name": "alsa_card.pci", "device.description": "Built-in Audio"},
            "params": {"Route": [{"index": 4, "direction": "Output", "device": 1, "description": "Headphones",
                "props": {"mute": true, "channelVolumes": [0.125, 0.125], "channelMap": ["FL", "FR"]}}]}}},
            {"id": 71, "type": "PipeWire:Interface:Node", "info": {"state": "idle", "props": {
                "node.name": "alsa_output.pci.analog-stereo", "media.class": "Audio/Sink",
                "device.id": 70, "card.profile.device": 1, "device.api": "alsa"},
                "params": {"Props": [{"mute": false, "channelVolumes": [1.0, 1.0]}]}}}]"#;
        g.apply(parse_dump(dev).unwrap());
        let n = &g.nodes[&71];
        assert_eq!(n.kind(), Kind::Output);
        let v = g.volume_of(n).unwrap();
        assert!(v.mute, "the route's volume wins over the node's");
        assert_eq!(v.channels, vec![0.125, 0.125]);
        assert_eq!(g.route_of(n).unwrap().1.index, 4);
    }

    #[test]
    fn metadata_values_as_text() {
        assert_eq!(metadata_name(&Value::String("{\"name\":\"a\"}".into())).as_deref(), Some("a"));
        assert_eq!(metadata_name(&Value::String("plain".into())).as_deref(), Some("plain"));
    }
}
