//! Shared state presentation for the window, CLI, and i3xrocks block.

use serde::{Deserialize, Serialize};

// External bar palette: semantic roles, tuned for Regolith's dark status bar.
// Keep product colors here rather than embedding literals in UI/render callers.
mod palette {
    pub const READY: &str = "#7dd3fc";
    pub const RECORDING: &str = "#fb7185";
    pub const PROCESSING: &str = "#c4b5fd";
    pub const SUCCESS: &str = "#86efac";
    pub const WARNING: &str = "#fcd34d";
    pub const MUTED: &str = "#94a3b8";
    pub const RECORDING_PULSE: [&str; 8] = [
        "#fb7185", "#fda4af", "#fecdd3", "#fda4af", "#fb7185", "#e85d75", "#ce4963", "#e85d75",
    ];
}
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Status {
    Ready,
    Recording(u64),
    Transcribing,
    Finished,
    PasteSkipped,
    Error,
    Offline,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct BarSnapshot {
    pub(crate) state: Status,
    pub(crate) animate: bool,
}
impl BarSnapshot {
    pub(crate) fn offline() -> Self {
        Self {
            state: Status::Offline,
            animate: false,
        }
    }
    pub(crate) fn markup(self, frame: usize) -> String {
        self.state.bar_frame(frame, self.animate)
    }
    pub(crate) fn is_animating(self) -> bool {
        self.animate && matches!(self.state, Status::Recording(_) | Status::Transcribing)
    }
}

impl Status {
    pub(crate) fn text(self) -> String {
        match self {
            Self::Ready => "MIC".into(),
            Self::Recording(seconds) => format!("REC {}:{:02}", seconds / 60, seconds % 60),
            Self::Transcribing => "STT …".into(),
            Self::Finished => "DONE".into(),
            Self::PasteSkipped => "READY • paste skipped".into(),
            Self::Error => "ERR".into(),
            Self::Offline => "OFF".into(),
        }
    }

    /// Static Pango icon, including when no animation is desired.
    pub(crate) fn bar(self) -> String {
        self.bar_frame(0, false)
    }

    /// No user/backend strings are interpolated; elapsed time is a bounded number.
    /// Font size follows the bar. Animation never changes the recording icon's width.
    pub(crate) fn bar_frame(self, frame: usize, animate: bool) -> String {
        let (icon, color) = match self {
            Self::Ready => ('\u{f130}', palette::READY),
            Self::Recording(_) => (
                '\u{f192}',
                if animate {
                    palette::RECORDING_PULSE[frame % palette::RECORDING_PULSE.len()]
                } else {
                    palette::RECORDING
                },
            ),
            Self::Transcribing => ('\u{f1ce}', palette::PROCESSING),
            Self::Finished => ('\u{f058}', palette::SUCCESS),
            Self::PasteSkipped => ('\u{f28b}', palette::WARNING),
            Self::Error => ('\u{f071}', palette::RECORDING),
            Self::Offline => ('\u{f131}', palette::MUTED),
        };
        if self == Self::Transcribing && animate {
            let spinner = SPINNER[frame % SPINNER.len()];
            return format!(
                "<span font_family=\"DejaVu Sans Mono\" foreground=\"{color}\">{spinner}</span>"
            );
        }
        let elapsed = match self {
            Self::Recording(seconds) => format!(
                " <span foreground=\"{}\">{}:{:02}</span>",
                palette::RECORDING,
                seconds / 60,
                seconds % 60
            ),
            _ => String::new(),
        };
        format!("<span font_family=\"FontAwesome\" foreground=\"{color}\">{icon}</span>{elapsed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_state_has_a_distinct_icon() {
        let states = [
            Status::Ready,
            Status::Recording(0),
            Status::Transcribing,
            Status::Finished,
            Status::PasteSkipped,
            Status::Error,
            Status::Offline,
        ];
        let icons: std::collections::HashSet<_> = states.into_iter().map(Status::bar).collect();
        assert_eq!(icons.len(), states.len());
    }
    #[test]
    fn recording_icon_includes_elapsed_time() {
        let output = Status::Recording(75).bar();
        assert!(output.contains('\u{f192}'));
        assert!(output.contains("1:15"));
    }
    #[test]
    fn plain_status_remains_readable_without_icon_font() {
        assert_eq!(Status::Recording(75).text(), "REC 1:15");
        assert_eq!(Status::Offline.text(), "OFF");
    }
    #[test]
    fn active_states_animate_and_other_states_stay_still() {
        for state in [Status::Recording(1), Status::Transcribing] {
            assert_ne!(state.bar_frame(0, true), state.bar_frame(1, true));
            assert_eq!(state.bar_frame(0, false), state.bar_frame(1, false));
        }
        for state in [
            Status::Ready,
            Status::Finished,
            Status::Error,
            Status::PasteSkipped,
            Status::Offline,
        ] {
            assert_eq!(state.bar_frame(0, true), state.bar_frame(1, true));
        }
    }
    #[test]
    fn animation_cycles_repeat_without_size_changes() {
        assert_eq!(
            Status::Transcribing.bar_frame(0, true),
            Status::Transcribing.bar_frame(10, true)
        );
        assert_eq!(
            Status::Recording(1).bar_frame(0, true),
            Status::Recording(1).bar_frame(8, true)
        );
    }
    #[test]
    fn bar_snapshot_round_trips_without_content_or_credentials() {
        let snapshot = BarSnapshot {
            state: Status::Recording(17),
            animate: true,
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: BarSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.state, Status::Recording(17));
        assert!(decoded.is_animating());
        assert!(!BarSnapshot::offline().is_animating());
    }
}
