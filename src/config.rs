//! Settings, card positions, scenes and effects, saved as JSON in
//! `~/.config/m3-sound/config.json`.

use crate::effects::Effect;
use crate::scenes::Scene;
use material_3_ui::theme::ThemeSettings;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub appearance: ThemeSettings,
    /// Cards the user moved, by unit key.
    pub positions: BTreeMap<String, [f32; 2]>,
    /// Cards the user hid, by unit key.
    pub hidden: BTreeSet<String>,
    pub scenes: Vec<Scene>,
    /// The scene kept connected, by name.
    pub active_scene: Option<String>,
    pub effects: Vec<Effect>,
    /// Draw moving dots on cords that carry sound.
    pub animate_cords: bool,
    /// One pin per channel instead of one jack per card.
    pub show_channels: bool,
    /// Show MIDI and video nodes.
    pub show_other: bool,
    /// Let sliders go to 150%.
    pub allow_boost: bool,
    /// Start effects and reconnect the active scene when the app opens.
    pub restore_on_start: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            appearance: ThemeSettings::default(),
            positions: BTreeMap::new(),
            hidden: BTreeSet::new(),
            scenes: Vec::new(),
            active_scene: None,
            effects: Vec::new(),
            animate_cords: true,
            show_channels: false,
            show_other: false,
            allow_boost: false,
            restore_on_start: true,
        }
    }
}

pub fn dir() -> PathBuf {
    material_3_ui::sys::paths::config_home().join("m3-sound")
}

pub fn path() -> PathBuf {
    dir().join("config.json")
}

/// Generated equalizer configurations and their logs.
pub fn effects_dir() -> PathBuf {
    material_3_ui::sys::paths::state_dir("m3-sound").join("effects")
}

impl Config {
    pub fn load() -> (Self, Option<String>) {
        Self::load_from(&path())
    }

    pub fn load_from(path: &std::path::Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(c) => (c, None),
                Err(e) => (Self::default(), Some(format!("Couldn't read {}: {e}", path.display()))),
            },
            Err(_) => (Self::default(), None),
        }
    }

    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        material_3_ui::fs::save_text(path, &text)
    }

    pub fn scene(&self, name: &str) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.name == name)
    }

    pub fn active_scene_mut(&mut self) -> Option<&mut Scene> {
        let name = self.active_scene.clone()?;
        self.scenes.iter_mut().find(|s| s.name == name)
    }

    /// The highest volume a slider goes to.
    pub fn max_volume(&self) -> f32 {
        if self.allow_boost {
            1.5
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/config.json");
        let mut c = Config::default();
        c.positions.insert("node:x".into(), [10.0, 20.0]);
        c.effects.push(Effect::new_equalizer("EQ"));
        c.scenes.push(Scene { name: "A".into(), links: vec![], exclusive: false });
        c.active_scene = Some("A".into());
        c.save_to(&path).unwrap();
        let (back, err) = Config::load_from(&path);
        assert!(err.is_none());
        assert_eq!(back, c);
        assert!(back.scene("A").is_some());

        std::fs::write(&path, r#"{"animate_cords": false}"#).unwrap();
        let (partial, err) = Config::load_from(&path);
        assert!(err.is_none());
        assert!(!partial.animate_cords);
        assert!(partial.restore_on_start);

        std::fs::write(&path, "not json").unwrap();
        let (_, err) = Config::load_from(&path);
        assert!(err.is_some());
    }
}
