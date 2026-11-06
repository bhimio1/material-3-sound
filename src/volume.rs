//! Slider values and PipeWire volumes. Sliders show the cube root of the
//! linear channel volume, like wpctl and pavucontrol, so 50% sounds about
//! half as loud. Balance moves sound between left and right channels.

use crate::graph::Volume;

pub fn to_slider(linear: f32) -> f32 {
    linear.max(0.0).cbrt()
}

pub fn to_linear(slider: f32) -> f32 {
    let s = slider.max(0.0);
    s * s * s
}

/// Which side a channel is on: -1 left, 1 right, 0 center (or mono).
pub fn side(channel: &str) -> i8 {
    match channel {
        "MONO" | "FC" | "LFE" | "RC" | "TC" | "TFC" | "TRC" | "" => 0,
        c if c.ends_with('L') || c.ends_with("LC") => -1,
        c if c.ends_with('R') || c.ends_with("RC") => 1,
        _ => 0,
    }
}

/// The loudest channel, in slider units.
pub fn master(v: &Volume) -> f32 {
    v.channels.iter().copied().map(to_slider).fold(0.0, f32::max)
}

/// -1 (all left) … 0 (centered) … 1 (all right).
pub fn balance(v: &Volume) -> f32 {
    let m = master(v);
    if m <= 0.0 {
        return 0.0;
    }
    let side_level = |s: i8| {
        v.channels
            .iter()
            .enumerate()
            .filter(|(i, _)| side(v.map.get(*i).map(String::as_str).unwrap_or("")) == s)
            .map(|(_, c)| to_slider(*c))
            .fold(None, |acc: Option<f32>, x| Some(acc.map_or(x, |a| a.max(x))))
    };
    match (side_level(-1), side_level(1)) {
        (Some(l), Some(r)) if r < l => -(1.0 - r / l.max(1e-6)),
        (Some(l), Some(r)) if l < r => 1.0 - l / r.max(1e-6),
        _ => 0.0,
    }
}

/// Linear channel volumes for a master level and balance.
pub fn channels_for(map: &[String], count: usize, master: f32, balance: f32) -> Vec<f32> {
    let b = balance.clamp(-1.0, 1.0);
    (0..count)
        .map(|i| {
            let gain = match side(map.get(i).map(String::as_str).unwrap_or("")) {
                -1 => 1.0 - b.max(0.0),
                1 => 1.0 + b.min(0.0),
                _ => 1.0,
            };
            to_linear(master * gain)
        })
        .collect()
}

/// A copy of `v` at a new master level, keeping the balance.
pub fn with_master(v: &Volume, master_level: f32) -> Vec<f32> {
    channels_for(&v.map, v.channels.len(), master_level, balance(v))
}

/// A copy of `v` with a new balance, keeping the master level.
pub fn with_balance(v: &Volume, bal: f32) -> Vec<f32> {
    channels_for(&v.map, v.channels.len(), master(v), bal)
}

/// "72%".
pub fn percent(slider: f32) -> String {
    format!("{:.0}%", slider * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(ch: &[f32]) -> Volume {
        Volume { channels: ch.to_vec(), mute: false, map: vec!["FL".into(), "FR".into()] }
    }

    #[test]
    fn cubic_round_trip() {
        assert!((to_slider(0.125) - 0.5).abs() < 1e-6);
        assert!((to_linear(0.5) - 0.125).abs() < 1e-6);
        assert_eq!(to_linear(-1.0), 0.0);
    }

    #[test]
    fn balance_round_trips() {
        for b in [-1.0, -0.4, 0.0, 0.3, 1.0f32] {
            let v = vol(&channels_for(&["FL".into(), "FR".into()], 2, 0.8, b));
            assert!((balance(&v) - b).abs() < 1e-4, "{b} -> {}", balance(&v));
            assert!((master(&v) - 0.8).abs() < 1e-4);
        }
        let v = vol(&[0.125, 0.125]);
        let louder = vol(&with_master(&v, 1.0));
        assert_eq!(louder.channels, vec![1.0, 1.0]);
        let left = vol(&with_balance(&v, -0.5));
        assert!(left.channels[0] > left.channels[1]);
        assert!((master(&left) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn sides_and_mono() {
        assert_eq!(side("FL"), -1);
        assert_eq!(side("RR"), 1);
        assert_eq!(side("FC"), 0);
        assert_eq!(side("MONO"), 0);
        let mono = Volume { channels: vec![0.5], mute: false, map: vec!["MONO".into()] };
        assert_eq!(balance(&mono), 0.0);
        assert!((with_balance(&mono, 1.0)[0] - 0.5).abs() < 1e-5);
    }
}
