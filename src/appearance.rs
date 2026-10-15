//! App-wide appearance policy; the saved preference remains owned by Controller.

use crate::settings::ThemePreference;
use gpui_kit::{
    App, WindowAppearance,
    component::{Theme, ThemeMode},
};

pub(crate) fn theme_mode(preference: ThemePreference, appearance: WindowAppearance) -> ThemeMode {
    match preference {
        ThemePreference::System => appearance.into(),
        ThemePreference::Light => ThemeMode::Light,
        ThemePreference::Dark => ThemeMode::Dark,
    }
}

/// Synchronizes every window, including when no application window is visible.
pub(crate) fn apply(preference: ThemePreference, cx: &mut App) {
    // GPUI Linux updates this app-wide value from XDG portal events, independently
    // of window lifetime. Missing portals / no preference fall back to Light.
    let mode = theme_mode(preference, cx.window_appearance());
    if Theme::global(cx).mode != mode {
        Theme::change(mode, None, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_follows_all_platform_appearances() {
        for (appearance, expected) in [
            (WindowAppearance::Light, ThemeMode::Light),
            (WindowAppearance::VibrantLight, ThemeMode::Light),
            (WindowAppearance::Dark, ThemeMode::Dark),
            (WindowAppearance::VibrantDark, ThemeMode::Dark),
        ] {
            assert_eq!(theme_mode(ThemePreference::System, appearance), expected);
        }
    }

    #[test]
    fn fixed_preferences_ignore_platform_changes() {
        for appearance in [
            WindowAppearance::Light,
            WindowAppearance::VibrantLight,
            WindowAppearance::Dark,
            WindowAppearance::VibrantDark,
        ] {
            assert_eq!(
                theme_mode(ThemePreference::Light, appearance),
                ThemeMode::Light
            );
            assert_eq!(
                theme_mode(ThemePreference::Dark, appearance),
                ThemeMode::Dark
            );
        }
    }
}
