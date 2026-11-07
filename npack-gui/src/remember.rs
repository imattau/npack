//! What the app remembers between runs: the tab you were on and the window
//! size. A small JSON file in the user's config directory, read leniently (a
//! missing or damaged file just means defaults) and written only when
//! something changed. It never holds anything sensitive.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const DEFAULT_SIZE: [f32; 2] = [1040.0, 720.0];
pub const MIN_SIZE: [f32; 2] = [720.0, 480.0];
/// Larger than any real screen today, so a corrupt value cannot ask for an
/// absurd window.
const MAX_SIZE: f32 = 8192.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Remembered {
    #[serde(default)]
    pub tab: Option<String>,
    #[serde(default)]
    pub size: Option<[f32; 2]>,
}

impl Remembered {
    /// The saved window size, kept within sensible bounds.
    pub fn window_size(&self) -> [f32; 2] {
        match self.size {
            Some([w, h]) if w.is_finite() && h.is_finite() => [
                w.clamp(MIN_SIZE[0], MAX_SIZE),
                h.clamp(MIN_SIZE[1], MAX_SIZE),
            ],
            _ => DEFAULT_SIZE,
        }
    }

    /// True when `size` differs enough from the saved one to be worth a write
    /// (resizing by dragging reports many tiny changes).
    pub fn size_changed(&self, size: [f32; 2]) -> bool {
        match self.size {
            Some([w, h]) => (w - size[0]).abs() > 2.0 || (h - size[1]).abs() > 2.0,
            None => true,
        }
    }
}

pub fn state_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("npack").join("gui-state.json"))
}

pub fn load() -> Remembered {
    state_path().map(|p| load_from(&p)).unwrap_or_default()
}

pub fn load_from(path: &std::path::Path) -> Remembered {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Best effort: failing to remember something is never worth interrupting
/// anyone for.
pub fn save(state: &Remembered) {
    if let Some(path) = state_path() {
        save_to(&path, state);
    }
}

pub fn save_to(path: &std::path::Path, state: &Remembered) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(state) {
        let temporary = path.with_extension("json.tmp");
        if std::fs::write(&temporary, text).is_ok() {
            let _ = std::fs::rename(&temporary, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_damaged_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_from(&dir.path().join("none.json")),
            Remembered::default()
        );
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{ not json").unwrap();
        assert_eq!(load_from(&bad), Remembered::default());
        assert_eq!(Remembered::default().window_size(), DEFAULT_SIZE);
    }

    #[test]
    fn state_round_trips_and_unknown_keys_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/state.json");
        let state = Remembered {
            tab: Some("browse".into()),
            size: Some([1200.0, 800.0]),
        };
        save_to(&path, &state);
        assert_eq!(load_from(&path), state);
        std::fs::write(&path, r#"{"tab":"health","future":1}"#).unwrap();
        assert_eq!(load_from(&path).tab.as_deref(), Some("health"));
    }

    #[test]
    fn a_silly_window_size_is_clamped() {
        let tiny = Remembered {
            tab: None,
            size: Some([10.0, 10.0]),
        };
        assert_eq!(tiny.window_size(), MIN_SIZE);
        let huge = Remembered {
            tab: None,
            size: Some([1e9, 1e9]),
        };
        assert_eq!(huge.window_size(), [MAX_SIZE, MAX_SIZE]);
        let nan = Remembered {
            tab: None,
            size: Some([f32::NAN, 500.0]),
        };
        assert_eq!(nan.window_size(), DEFAULT_SIZE);
    }

    #[test]
    fn only_a_real_resize_counts_as_a_change() {
        let state = Remembered {
            tab: None,
            size: Some([1000.0, 700.0]),
        };
        assert!(!state.size_changed([1001.0, 699.0]));
        assert!(state.size_changed([1040.0, 700.0]));
        assert!(Remembered::default().size_changed([1000.0, 700.0]));
    }
}
