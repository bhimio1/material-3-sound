//! Effects the app creates: parametric equalizers (PipeWire filter-chains
//! of biquads, run as their own `pipewire -c` process) and mix buses
//! (lingering null sinks whose monitor can be patched onward).

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Every node the app creates is named `m3sound.…`.
pub const NODE_PREFIX: &str = "m3sound.";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub kind: EffectKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum EffectKind {
    Equalizer(Equalizer),
    Bus(Bus),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bus {
    pub channels: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Equalizer {
    /// Gain applied before the bands, in dB (negative to make headroom).
    pub preamp: f32,
    pub bands: Vec<Band>,
    pub channels: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Band {
    pub kind: BandKind,
    pub freq: f32,
    pub gain: f32,
    pub q: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandKind {
    LowShelf,
    Peaking,
    HighShelf,
    LowPass,
    HighPass,
    Notch,
}

impl BandKind {
    pub const ALL: [BandKind; 6] =
        [BandKind::LowShelf, BandKind::Peaking, BandKind::HighShelf, BandKind::LowPass, BandKind::HighPass, BandKind::Notch];

    /// The filter-chain builtin.
    pub fn label(self) -> &'static str {
        match self {
            BandKind::LowShelf => "bq_lowshelf",
            BandKind::Peaking => "bq_peaking",
            BandKind::HighShelf => "bq_highshelf",
            BandKind::LowPass => "bq_lowpass",
            BandKind::HighPass => "bq_highpass",
            BandKind::Notch => "bq_notch",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BandKind::LowShelf => "Low shelf",
            BandKind::Peaking => "Peak",
            BandKind::HighShelf => "High shelf",
            BandKind::LowPass => "Low pass",
            BandKind::HighPass => "High pass",
            BandKind::Notch => "Notch",
        }
    }

    /// Whether the band's gain does anything.
    pub fn has_gain(self) -> bool {
        matches!(self, BandKind::LowShelf | BandKind::Peaking | BandKind::HighShelf)
    }
}

pub const DEFAULT_FREQS: [f32; 6] = [80.0, 200.0, 600.0, 1800.0, 5000.0, 10000.0];

impl Default for Equalizer {
    fn default() -> Self {
        let bands = DEFAULT_FREQS
            .iter()
            .enumerate()
            .map(|(i, &freq)| {
                let kind = match i {
                    0 => BandKind::LowShelf,
                    5 => BandKind::HighShelf,
                    _ => BandKind::Peaking,
                };
                Band { kind, freq, gain: 0.0, q: if kind == BandKind::Peaking { 1.0 } else { 0.7 } }
            })
            .collect();
        Self { preamp: 0.0, bands, channels: 2 }
    }
}

/// Named starting points: preamp and the six default bands' gains.
pub const PRESETS: &[(&str, f32, [f32; 6])] = &[
    ("Flat", 0.0, [0.0; 6]),
    ("Bass boost", -6.0, [6.0, 2.0, 0.0, 0.0, 0.0, 0.0]),
    ("Treble boost", -5.0, [0.0, 0.0, 0.0, 0.0, 2.0, 5.0]),
    ("Vocal clarity", -3.0, [-3.0, -2.0, 0.0, 3.0, 2.0, 0.0]),
    ("Loudness", -5.0, [5.0, 0.0, -1.0, 0.0, 0.0, 4.0]),
    ("Warm", -3.0, [3.0, 1.5, 0.0, 0.0, 0.0, -2.0]),
    ("Podcast", -2.0, [-4.0, -1.0, 1.0, 2.0, 1.0, -1.0]),
];

impl Equalizer {
    pub fn preset(name: &str) -> Option<Self> {
        let (_, preamp, gains) = PRESETS.iter().find(|(n, _, _)| *n == name)?;
        let mut eq = Self { preamp: *preamp, ..Self::default() };
        for (b, g) in eq.bands.iter_mut().zip(gains) {
            b.gain = *g;
        }
        Some(eq)
    }

    /// The preamp that keeps the loudest boost from clipping.
    pub fn auto_preamp(&self) -> f32 {
        let peak = (0..200)
            .map(|i| log_freq(i as f32 / 199.0))
            .map(|f| self.response_db(f, 48000.0) - self.preamp)
            .fold(0.0f32, f32::max);
        -(peak * 10.0).round() / 10.0
    }

    /// The control values PipeWire knows the bands by.
    pub fn controls(&self) -> Vec<(String, f32)> {
        let mut v = vec![("preamp:Gain".to_string(), self.preamp)];
        for (i, b) in self.bands.iter().enumerate() {
            let n = i + 1;
            v.push((format!("band{n}:Freq"), b.freq));
            v.push((format!("band{n}:Q"), b.q));
            v.push((format!("band{n}:Gain"), b.gain));
        }
        v
    }

    /// Whether `other` differs only in values PipeWire can change live
    /// (frequencies, gains and Q), not in the graph's shape.
    pub fn same_shape(&self, other: &Equalizer) -> bool {
        self.channels == other.channels
            && self.bands.len() == other.bands.len()
            && self.bands.iter().zip(&other.bands).all(|(a, b)| a.kind == b.kind)
    }

    /// The whole curve at `freq`, in dB.
    pub fn response_db(&self, freq: f32, rate: f32) -> f32 {
        self.preamp + self.bands.iter().map(|b| band_response_db(b, freq, rate)).sum::<f32>()
    }
}

/// One band's gain at `freq` (RBJ cookbook biquads, as filter-chain runs them).
pub fn band_response_db(b: &Band, freq: f32, rate: f32) -> f32 {
    let (fs, f0) = (rate as f64, (b.freq as f64).clamp(1.0, rate as f64 * 0.499));
    let w0 = 2.0 * PI * f0 / fs;
    let (cw, sw) = (w0.cos(), w0.sin());
    let q = (b.q as f64).max(0.05);
    let alpha = sw / (2.0 * q);
    let a = 10f64.powf(b.gain as f64 / 40.0);
    let sa = a.sqrt();
    let (b0, b1, b2, a0, a1, a2) = match b.kind {
        BandKind::Peaking => (1.0 + alpha * a, -2.0 * cw, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cw, 1.0 - alpha / a),
        BandKind::LowShelf => (
            a * ((a + 1.0) - (a - 1.0) * cw + 2.0 * sa * alpha),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
            a * ((a + 1.0) - (a - 1.0) * cw - 2.0 * sa * alpha),
            (a + 1.0) + (a - 1.0) * cw + 2.0 * sa * alpha,
            -2.0 * ((a - 1.0) + (a + 1.0) * cw),
            (a + 1.0) + (a - 1.0) * cw - 2.0 * sa * alpha,
        ),
        BandKind::HighShelf => (
            a * ((a + 1.0) + (a - 1.0) * cw + 2.0 * sa * alpha),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
            a * ((a + 1.0) + (a - 1.0) * cw - 2.0 * sa * alpha),
            (a + 1.0) - (a - 1.0) * cw + 2.0 * sa * alpha,
            2.0 * ((a - 1.0) - (a + 1.0) * cw),
            (a + 1.0) - (a - 1.0) * cw - 2.0 * sa * alpha,
        ),
        BandKind::LowPass => ((1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
        BandKind::HighPass => ((1.0 + cw) / 2.0, -(1.0 + cw), (1.0 + cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
        BandKind::Notch => (1.0, -2.0 * cw, 1.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
    };
    let w = 2.0 * PI * (freq as f64) / fs;
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let num = (b0 + b1 * c1 + b2 * c2).powi(2) + (b1 * s1 + b2 * s2).powi(2);
    let den = (a0 + a1 * c1 + a2 * c2).powi(2) + (a1 * s1 + a2 * s2).powi(2);
    (10.0 * (num / den.max(1e-30)).log10()).clamp(-120.0, 60.0) as f32
}

pub const MIN_FREQ: f32 = 20.0;
pub const MAX_FREQ: f32 = 20000.0;

/// 0..1 across the audible range, logarithmically.
pub fn log_freq(t: f32) -> f32 {
    MIN_FREQ * (MAX_FREQ / MIN_FREQ).powf(t.clamp(0.0, 1.0))
}

pub fn freq_t(freq: f32) -> f32 {
    ((freq.max(1.0) / MIN_FREQ).ln() / (MAX_FREQ / MIN_FREQ).ln()).clamp(0.0, 1.0)
}

/// "1.8 kHz", "80 Hz".
pub fn freq_label(f: f32) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{:.0} kHz", k)
        } else {
            format!("{:.1} kHz", k)
        }
    } else {
        format!("{:.0} Hz", f)
    }
}

impl Effect {
    pub fn new_equalizer(name: &str) -> Self {
        Self { id: new_id(), name: name.into(), enabled: true, kind: EffectKind::Equalizer(Equalizer::default()) }
    }

    pub fn new_bus(name: &str) -> Self {
        Self { id: new_id(), name: name.into(), enabled: true, kind: EffectKind::Bus(Bus { channels: 2 }) }
    }

    /// The node others connect into: the equalizer's input side or the bus.
    pub fn node_name(&self) -> String {
        match self.kind {
            EffectKind::Equalizer(_) => format!("{NODE_PREFIX}eq.{}", self.id),
            EffectKind::Bus(_) => format!("{NODE_PREFIX}bus.{}", self.id),
        }
    }

    /// The equalizer's output side.
    pub fn output_node_name(&self) -> Option<String> {
        matches!(self.kind, EffectKind::Equalizer(_)).then(|| format!("{}.out", self.node_name()))
    }

    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            EffectKind::Equalizer(_) => "Equalizer",
            EffectKind::Bus(_) => "Mix bus",
        }
    }

    /// `pw-cli create-node adapter` properties for a bus.
    pub fn bus_props(&self) -> Option<String> {
        let EffectKind::Bus(bus) = &self.kind else { return None };
        Some(format!(
            "{{ factory.name=support.null-audio-sink node.name=\"{}\" node.description=\"{}\" media.class=Audio/Sink \
             object.linger=true monitor.channel-volumes=true audio.position=[ {} ] }}",
            self.node_name(),
            spa_escape(&self.name),
            positions(bus.channels)
        ))
    }

    /// A complete `pipewire -c` configuration running the equalizer.
    pub fn filter_chain_conf(&self) -> Option<String> {
        let EffectKind::Equalizer(eq) = &self.kind else { return None };
        let mut nodes = vec![format!(
            "                    {{ type = builtin name = preamp label = bq_highshelf control = {{ \"Freq\" = 0.0 \"Q\" = 1.0 \"Gain\" = {:.2} }} }}",
            eq.preamp
        )];
        let mut links = Vec::new();
        let mut prev = "preamp".to_string();
        for (i, b) in eq.bands.iter().enumerate() {
            let name = format!("band{}", i + 1);
            nodes.push(format!(
                "                    {{ type = builtin name = {name} label = {} control = {{ \"Freq\" = {:.2} \"Q\" = {:.3} \"Gain\" = {:.2} }} }}",
                b.kind.label(),
                b.freq,
                b.q,
                b.gain
            ));
            links.push(format!("                    {{ output = \"{prev}:Out\" input = \"{name}:In\" }}"));
            prev = name;
        }
        Some(format!(
            r#"# Generated by Material 3 Sound for the equalizer "{title}". Edits are overwritten.
context.properties = {{
    log.level = 0
}}
context.spa-libs = {{
    audio.convert.* = audioconvert/libspa-audioconvert
    support.*       = support/libspa-support
}}
context.modules = [
    {{ name = libpipewire-module-rt args = {{ }} flags = [ ifexists nofail ] }}
    {{ name = libpipewire-module-protocol-native }}
    {{ name = libpipewire-module-client-node }}
    {{ name = libpipewire-module-adapter }}
    {{ name = libpipewire-module-filter-chain
        args = {{
            node.description = "{title}"
            media.name = "{title}"
            filter.graph = {{
                nodes = [
{nodes}
                ]
                links = [
{links}
                ]
            }}
            audio.channels = {channels}
            audio.position = [ {positions} ]
            capture.props = {{
                node.name = "{input}"
                media.class = Audio/Sink
            }}
            playback.props = {{
                node.name = "{output}"
                node.passive = true
            }}
        }}
    }}
]
"#,
            title = spa_escape(&self.name),
            nodes = nodes.join("\n"),
            links = links.join("\n"),
            channels = eq.channels,
            positions = positions(eq.channels),
            input = self.node_name(),
            output = self.output_node_name().unwrap_or_default(),
        ))
    }
}

/// The effect a node belongs to, by its name.
pub fn effect_for_node<'a>(effects: &'a [Effect], node_name: &str) -> Option<&'a Effect> {
    effects
        .iter()
        .find(|e| e.node_name() == node_name || e.output_node_name().as_deref() == Some(node_name))
}

fn positions(channels: u32) -> &'static str {
    match channels {
        1 => "MONO",
        6 => "FL FR FC LFE RL RR",
        8 => "FL FR FC LFE RL RR SL SR",
        _ => "FL FR",
    }
}

/// A string safe inside SPA JSON quotes.
fn spa_escape(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).map(|c| if c == '"' || c == '\\' { '\'' } else { c }).collect()
}

/// A short id for node names: base-36 of the current time.
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static BUMP: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        .wrapping_add(BUMP.fetch_add(7919, Ordering::Relaxed));
    let mut n = nanos / 1000;
    let mut s = String::new();
    while n > 0 && s.len() < 7 {
        let d = (n % 36) as u8;
        s.push(if d < 10 { (b'0' + d) as char } else { (b'a' + d - 10) as char });
        n /= 36;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_is_flat_and_boosts_show() {
        let eq = Equalizer::default();
        for f in [30.0, 200.0, 1000.0, 9000.0] {
            assert!(eq.response_db(f, 48000.0).abs() < 0.01, "{f}");
        }
        let mut eq = Equalizer::default();
        eq.bands[2].gain = 6.0;
        let at = eq.response_db(eq.bands[2].freq, 48000.0);
        assert!((at - 6.0).abs() < 0.05, "peak gain at its center: {at}");
        assert!(eq.response_db(20.0, 48000.0).abs() < 0.5);
        assert!((eq.auto_preamp() + 6.0).abs() < 0.2);
        let ls = Band { kind: BandKind::LowShelf, freq: 100.0, gain: 6.0, q: 0.7 };
        assert!((band_response_db(&ls, 20.0, 48000.0) - 6.0).abs() < 0.5);
        assert!(band_response_db(&ls, 5000.0, 48000.0).abs() < 0.1);
        let lp = Band { kind: BandKind::LowPass, freq: 1000.0, gain: 0.0, q: 0.707 };
        assert!(band_response_db(&lp, 8000.0, 48000.0) < -30.0);
    }

    #[test]
    fn presets_and_shapes() {
        let bass = Equalizer::preset("Bass boost").unwrap();
        assert_eq!(bass.bands[0].gain, 6.0);
        assert!(bass.same_shape(&Equalizer::default()));
        let mut other = Equalizer::default();
        other.bands[1].kind = BandKind::Notch;
        assert!(!other.same_shape(&bass));
        assert!(Equalizer::preset("nope").is_none());
        let c = bass.controls();
        assert_eq!(c[0], ("preamp:Gain".into(), -6.0));
        assert!(c.contains(&("band1:Gain".into(), 6.0)));
        assert_eq!(c.len(), 1 + 6 * 3);
    }

    #[test]
    fn conf_and_names() {
        let mut e = Effect::new_equalizer("My \"EQ\"");
        e.id = "abc".into();
        assert_eq!(e.node_name(), "m3sound.eq.abc");
        assert_eq!(e.output_node_name().as_deref(), Some("m3sound.eq.abc.out"));
        let conf = e.filter_chain_conf().unwrap();
        assert!(conf.contains("node.name = \"m3sound.eq.abc\""));
        assert!(conf.contains("label = bq_peaking"));
        assert!(conf.contains("{ output = \"band5:Out\" input = \"band6:In\" }"));
        assert!(conf.contains("node.description = \"My 'EQ'\""));
        assert!(e.bus_props().is_none());
        let bus = Effect::new_bus("Stream mix");
        assert!(bus.bus_props().unwrap().contains("media.class=Audio/Sink"));
        assert!(bus.filter_chain_conf().is_none());
        assert_eq!(effect_for_node(std::slice::from_ref(&e), "m3sound.eq.abc.out").map(|x| x.id.as_str()), Some("abc"));
        assert_ne!(new_id(), new_id());
    }

    #[test]
    fn frequency_axis() {
        assert!((log_freq(0.0) - 20.0).abs() < 1e-3);
        assert!((log_freq(1.0) - 20000.0).abs() < 1.0);
        assert!((freq_t(log_freq(0.37)) - 0.37).abs() < 1e-4);
        assert_eq!(freq_label(1800.0), "1.8 kHz");
        assert_eq!(freq_label(10000.0), "10 kHz");
        assert_eq!(freq_label(80.0), "80 Hz");
    }
}
