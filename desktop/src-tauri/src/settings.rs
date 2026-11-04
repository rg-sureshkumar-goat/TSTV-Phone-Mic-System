//! Settings remembered between launches (JSON in the app's config folder).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::MAX_SLOTS;

pub const FIRST_PHONE_CHANNEL: u32 = 17;

/// A slot reserved for one phone, so it returns to the same channel after dropping.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SlotLock {
    pub device_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct SlotSettings {
    pub channel: u32,
    pub gain_db: f32,
    pub lock: Option<SlotLock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Output device the operator picked. `None` = StudioLive if present, else system default.
    pub device: Option<String>,
    /// Jitter buffer target in ms.
    pub buffer_ms: f32,
    pub slots: Vec<SlotSettings>,
    /// Phones must give the join code to connect.
    pub require_code: bool,
    pub join_code: String,
    /// Names the operator has given talent, by phone.
    pub names: HashMap<String, String>,
}

impl Default for SlotSettings {
    fn default() -> Self {
        Self { channel: FIRST_PHONE_CHANNEL, gain_db: 0.0, lock: None }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device: None,
            buffer_ms: 40.0,
            slots: (0..MAX_SLOTS as u32)
                .map(|i| SlotSettings { channel: FIRST_PHONE_CHANNEL + i, ..Default::default() })
                .collect(),
            require_code: false,
            join_code: String::new(),
            names: HashMap::new(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        let mut s: Settings = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let defaults = Settings::default();
        s.slots.truncate(MAX_SLOTS);
        while s.slots.len() < MAX_SLOTS {
            s.slots.push(defaults.slots[s.slots.len()].clone());
        }
        s.buffer_ms = s.buffer_ms.clamp(MIN_BUFFER_MS, MAX_BUFFER_MS);
        s
    }

    pub fn save(&self, path: &Path) {
        let text = serde_json::to_string_pretty(self).expect("settings serialize");
        // Write then rename, so a crash never leaves a half-written file.
        let tmp: PathBuf = path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)) {
            log::warn!("Couldn't save settings to {}: {e}", path.display());
        }
    }
}

pub const MIN_BUFFER_MS: f32 = 10.0;
pub const MAX_BUFFER_MS: f32 = 200.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_partial_file_gets_defaults() {
        let dir = std::env::temp_dir().join(format!("tstv-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());

        std::fs::write(&path, r#"{"bufferMs": 5, "slots": [{"channel": 30}]}"#).unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.buffer_ms, MIN_BUFFER_MS);
        assert_eq!(s.slots.len(), MAX_SLOTS);
        assert_eq!(s.slots[0].channel, 30);
        assert_eq!(s.slots[1].channel, 18);

        let mut s = s;
        s.require_code = true;
        s.join_code = "4821".into();
        s.names.insert("phone-a".into(), "Alex".into());
        s.slots[2].lock = Some(SlotLock { device_id: "phone-a".into(), name: "Alex".into() });
        s.save(&path);
        assert_eq!(Settings::load(&path), s);
        std::fs::remove_dir_all(&dir).ok();
    }
}
