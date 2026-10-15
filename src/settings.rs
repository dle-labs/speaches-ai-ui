use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub(crate) base_url: String,
    pub(crate) model: String,
    pub(crate) language: String,
    pub(crate) device: String,
    pub(crate) copy: bool,
    pub(crate) paste: bool,
    pub(crate) chime: bool,
    pub(crate) terminal_paste: bool,
    pub(crate) quit_on_ctrl_q: bool,
    pub(crate) toggle_shortcut: String,
    pub(crate) cancel_shortcut: String,
    pub(crate) language_shortcut: String,
    pub(crate) quick_languages: Vec<String>,
    pub(crate) animate_bar: bool,
    pub(crate) theme: ThemePreference,
    #[serde(skip)]
    pub(crate) api_key: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:9791".into(),
            model: "deepdml/faster-whisper-large-v3-turbo-ct2".into(),
            language: String::new(),
            device: String::new(),
            api_key: String::new(),
            copy: false,
            paste: false,
            chime: false,
            terminal_paste: false,
            quit_on_ctrl_q: false,
            toggle_shortcut: "Ctrl+Alt+Space".into(),
            cancel_shortcut: "Ctrl+Shift+X".into(),
            language_shortcut: "Ctrl+Alt+Shift+L".into(),
            quick_languages: crate::language::default_quick_languages(),
            animate_bar: true,
            theme: ThemePreference::default(),
        }
    }
}
impl Settings {
    fn path() -> Result<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .context("No user configuration directory available")?;
        Ok(base.join("speaches-ai/settings.json"))
    }
    pub(crate) fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(path)?;
        anyhow::ensure!(bytes.len() <= 64 * 1024, "Settings file exceeds size limit");
        let mut settings: Self = serde_json::from_slice(&bytes)?;
        // Keep manually edited favorites bounded and unique before they become UI IDs.
        settings.quick_languages =
            crate::language::validate_quick_languages(&settings.quick_languages)
                .unwrap_or_else(|_| crate::language::default_quick_languages());
        if let Ok(code) = crate::language::normalize(&settings.language) {
            settings.language = code;
        }
        Ok(settings)
    }
    pub(crate) fn save(&self) -> Result<()> {
        let path = Self::path()?;
        let parent = path.parent().context("Invalid settings path")?;
        fs::create_dir_all(parent)?;
        let temp = path.with_extension("json.new");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.sync_all()?;
        fs::rename(temp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_are_never_serialized() {
        let s = Settings {
            api_key: "private-secret".into(),
            ..Settings::default()
        };
        assert!(
            !serde_json::to_string(&s)
                .unwrap()
                .contains("private-secret")
        );
    }
    #[test]
    fn old_settings_receive_default_shortcut() {
        let settings: Settings = serde_json::from_str(r#"{"copy":true}"#).unwrap();
        assert_eq!(settings.toggle_shortcut, "Ctrl+Alt+Space");
        assert_eq!(settings.cancel_shortcut, "Ctrl+Shift+X");
        assert_eq!(settings.language_shortcut, "Ctrl+Alt+Shift+L");
        assert_eq!(
            settings.quick_languages,
            crate::language::default_quick_languages()
        );
    }
    #[test]
    fn custom_shortcut_round_trips() {
        let settings = Settings {
            toggle_shortcut: "Super+Shift+F9".into(),
            cancel_shortcut: "Super+Shift+F10".into(),
            language_shortcut: "Super+Shift+F11".into(),
            quick_languages: vec!["en".into(), "fr".into()],
            ..Settings::default()
        };
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored.toggle_shortcut, settings.toggle_shortcut);
        assert_eq!(restored.cancel_shortcut, settings.cancel_shortcut);
        assert_eq!(restored.language_shortcut, settings.language_shortcut);
        assert_eq!(restored.quick_languages, settings.quick_languages);
    }
    #[test]
    fn animation_defaults_on_and_respects_saved_reduced_motion() {
        assert!(serde_json::from_str::<Settings>("{}").unwrap().animate_bar);
        let settings: Settings = serde_json::from_str(r#"{"animate_bar":false}"#).unwrap();
        assert!(!settings.animate_bar);
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert!(!restored.animate_bar);
    }
    #[test]
    fn chime_is_opt_in_and_saved_preference_is_preserved() {
        assert!(!Settings::default().chime);
        let saved: Settings = serde_json::from_str(r#"{"chime":true}"#).unwrap();
        assert!(saved.chime);
        let saved_off: Settings = serde_json::from_str(r#"{"chime":false}"#).unwrap();
        assert!(!saved_off.chime);
    }
    #[test]
    fn legacy_settings_default_to_system_theme() {
        let settings: Settings = serde_json::from_str(r#"{"copy":true}"#).unwrap();
        assert_eq!(settings.theme, ThemePreference::System);
        assert!(settings.copy);
    }
    #[test]
    fn theme_preferences_round_trip_as_snake_case() {
        for (preference, name) in [
            (ThemePreference::System, "system"),
            (ThemePreference::Light, "light"),
            (ThemePreference::Dark, "dark"),
        ] {
            let settings = Settings {
                theme: preference,
                ..Settings::default()
            };
            let encoded = serde_json::to_value(&settings).unwrap();
            assert_eq!(encoded["theme"], name);
            let restored: Settings = serde_json::from_value(encoded).unwrap();
            assert_eq!(restored.theme, preference);
        }
    }
    #[test]
    fn invalid_theme_is_rejected_instead_of_silently_reset() {
        assert!(serde_json::from_str::<Settings>(r#"{"theme":"blue"}"#).is_err());
    }
    #[test]
    fn output_defaults_do_not_modify_clipboard_or_type() {
        let s = Settings::default();
        assert!(!s.copy && !s.paste);
    }
}
