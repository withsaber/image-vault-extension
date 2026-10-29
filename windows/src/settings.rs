//! Persisted preferences (the Mac app's UserDefaults keys), stored as
//! settings.json next to the vault. Reads the old C# build's keys too.

use serde::{Deserialize, Serialize};

use crate::vault;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Position {
    Left,
    #[default]
    Center,
    Right,
}

impl Position {
    pub const ALL: [Position; 3] = [Position::Left, Position::Center, Position::Right];
    pub fn title(self) -> &'static str {
        match self {
            Position::Left => "Left",
            Position::Center => "Center",
            Position::Right => "Right",
        }
    }
}

fn yes() -> bool {
    true
}
fn five() -> u32 {
    5
}
fn ben() -> Vec<String> {
    vec!["ben".into()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default, alias = "Position")]
    pub island_position: Position,
    /// Monitor device name (\\.\DISPLAY1); None = primary.
    #[serde(default, alias = "Display")]
    pub island_display: Option<String>,
    #[serde(default = "yes")]
    pub show_notch: bool,
    #[serde(default = "yes", alias = "SaveClipboard")]
    pub save_clipboard: bool,
    #[serde(default = "yes", alias = "SaveScreenshots")]
    pub save_screenshots: bool,
    #[serde(default = "yes")]
    pub clipboard_history: bool,
    #[serde(default = "yes")]
    pub music_controls: bool,
    #[serde(default = "ben")]
    pub ocr_extra_languages: Vec<String>,
    /// Virtual-key + MOD_* flags; 0 = default.
    #[serde(default)]
    pub ocr_hot_key_code: u32,
    #[serde(default)]
    pub ocr_hot_key_modifiers: u32,
    /// How long a screenshot's preview card stays (seconds); 0 = until closed.
    #[serde(default = "five")]
    pub shot_preview_seconds: u32,
    #[serde(default)]
    pub shot_hot_key_code: u32,
    #[serde(default)]
    pub shot_hot_key_modifiers: u32,
}

impl Default for Settings {
    fn default() -> Self {
        serde_json::from_str("{}").unwrap()
    }
}

impl Settings {
    fn path() -> std::path::PathBuf {
        vault::root().join("settings.json")
    }

    pub fn load() -> Settings {
        std::fs::read(Self::path()).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let tmp = Self::path().with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, Self::path());
            }
        }
    }
}

/// "Open at Login" via HKCU\...\Run.
pub mod autostart {
    use windows::Win32::System::Registry::*;
    use windows::core::{PCWSTR, w};

    const KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    const NAME: PCWSTR = w!("ImageVault");

    pub fn enabled() -> bool {
        unsafe {
            let mut size = 0u32;
            RegGetValueW(HKEY_CURRENT_USER, KEY, NAME, RRF_RT_REG_SZ, None, None, Some(&mut size)).is_ok()
        }
    }

    pub fn set(on: bool) -> bool {
        unsafe {
            let mut key = HKEY::default();
            if RegCreateKeyExW(HKEY_CURRENT_USER, KEY, None, None, REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None)
                .is_err()
            {
                return false;
            }
            let ok = if on {
                let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
                let v = crate::util::wide(&format!("\"{exe}\""));
                let bytes = std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2);
                RegSetValueExW(key, NAME, None, REG_SZ, Some(bytes)).is_ok()
            } else {
                let r = RegDeleteValueW(key, NAME);
                r.is_ok() || r == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND
            };
            let _ = RegCloseKey(key);
            ok
        }
    }
}
