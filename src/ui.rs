use crate::{
    audio,
    controller::Controller,
    language,
    settings::{Settings, ThemePreference},
    status::Status,
};
use gpui_kit::component::{
    ActiveTheme, Disableable, IndexPath, Selectable, Sizable,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    menu::DropdownMenu,
    progress::Progress,
    radio::RadioGroup,
    select::{SearchableVec, Select, SelectEvent, SelectState},
    tab::{Tab, TabBar},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

actions!(
    speaches,
    [
        CloseOrQuit,
        QuitApplication,
        OpenSettings,
        ShowDictation,
        SaveSettings
    ]
);

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkspaceTab {
    Dictation,
    Settings,
}

pub(crate) fn show(controller: Entity<Controller>, cx: &mut App) {
    if let Some(handle) = controller.read(cx).window
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let owner = controller.clone();
    // Initial native window bounds are a platform boundary; inner layout uses theme/rem helpers.
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(760.), px(760.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Speaches AI".into()),
            ..Default::default()
        }),
        // Native minimum bounds are physical window-system constraints.
        window_min_size: Some(size(px(540.), px(540.))),
        app_id: Some("speaches-ai".into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, move |window, cx| {
        let closed = owner.clone();
        window.on_window_should_close(cx, move |_, cx| {
            closed.update(cx, |c, _| c.window = None);
            true
        });
        cx.new(|cx| DictationView::new(owner, window, cx))
    }) {
        Ok((handle, _)) => controller.update(cx, |c, _| c.window = Some(handle)),
        Err(e) => controller.update(cx, |c, cx| {
            c.notice = format!("Could not open window: {e}");
            cx.notify();
        }),
    }
}

struct DictationView {
    controller: Entity<Controller>,
    url: Entity<InputState>,
    key: Entity<InputState>,
    language: Entity<InputState>,
    shortcut: Entity<InputState>,
    cancel_shortcut: Entity<InputState>,
    language_shortcut: Entity<InputState>,
    quick_languages: Entity<InputState>,
    validation_error: Option<String>,
    last_language: String,
    last_model: String,
    language_revision: u64,
    model_selection_revision: u64,
    model: Entity<SelectState<SearchableVec<String>>>,
    device: Entity<SelectState<Vec<String>>>,
    transcript: Entity<TextareaState>,
    known_models_revision: u64,
    last_transcript: String,
    options: Settings,
    retry_confirm: Option<(u64, Settings)>,
    focus: FocusHandle,
    tab: WorkspaceTab,
    form_feedback: Option<String>,
    form_error: bool,
    copied: bool,
    _subscriptions: Vec<Subscription>,
}
impl DictationView {
    fn new(controller: Entity<Controller>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = controller.read(cx).settings.clone();
        let known_models_revision = controller.read(cx).models_revision;
        let language_revision = controller.read(cx).language_revision;
        let model_selection_revision = controller.read(cx).model_selection_revision;
        let mut models = controller.read(cx).models.clone();
        if !models.contains(&settings.model) {
            models.insert(0, settings.model.clone());
        }
        let chosen = models
            .iter()
            .position(|m| m == &settings.model)
            .map(IndexPath::new);
        let model = cx.new(|cx| {
            SelectState::new(SearchableVec::new(models), chosen, window, cx).searchable(true)
        });
        let mut devices = vec!["System default".to_owned()];
        devices.extend(audio::devices().unwrap_or_default());
        let chosen = devices
            .iter()
            .position(|d| d == &settings.device)
            .unwrap_or(0);
        let device =
            cx.new(|cx| SelectState::new(devices, Some(IndexPath::new(chosen)), window, cx));
        let text = controller.read(cx).transcript.clone();
        let transcript = cx.new(|cx| TextareaState::new(window, cx).default_value(text.clone()));
        let _subscription = cx.observe_in(&controller, window, |this, _, window, cx| {
            let c = this.controller.read(cx);
            let language = c.settings.language.clone();
            let model = c.settings.model.clone();
            let language_revision = c.language_revision;
            let model_selection_revision = c.model_selection_revision;
            let models_revision = c.models_revision;
            let mut models = if models_revision != this.known_models_revision
                || model != this.last_model
                || model_selection_revision != this.model_selection_revision
            {
                c.models.clone()
            } else {
                Vec::new()
            };
            let text = c.transcript.clone();
            if this
                .retry_confirm
                .as_ref()
                .is_some_and(|(revision, settings)| {
                    *revision != c.held_revision
                        || settings != &c.settings
                        || c.held_audio.is_none()
                })
            {
                this.retry_confirm = None;
            }
            // Sync only externally changed selections, never unrelated unsaved fields.
            if language != this.last_language || language_revision != this.language_revision {
                this.language_revision = language_revision;
                this.last_language = language.clone();
                this.options.language = language.clone();
                this.language
                    .update(cx, |state, cx| state.set_value(language, window, cx));
            }
            let model_changed = model != this.last_model
                || model_selection_revision != this.model_selection_revision;
            this.model_selection_revision = model_selection_revision;
            if models_revision != this.known_models_revision || model_changed {
                this.known_models_revision = models_revision;
                let draft = this.model.read(cx).selected_value().cloned();
                for value in draft.iter().chain(std::iter::once(&model)) {
                    if !models.contains(value) {
                        models.push(value.clone());
                    }
                }
                let selected = if model_changed {
                    Some(model.clone())
                } else {
                    draft
                };
                this.last_model = model.clone();
                this.options.model = model;
                this.model.update(cx, |state, cx| {
                    state.set_items(SearchableVec::new(models), window, cx);
                    if let Some(value) = selected {
                        state.set_selected_value(&value, window, cx);
                    }
                    cx.notify();
                });
            }
            if text != this.last_transcript {
                this.copied = false;
                this.last_transcript = text.clone();
                this.transcript
                    .update(cx, |state, cx| state.set_value(text, window, cx));
            }
            cx.notify();
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut view = Self {
            controller,
            url: cx.new(|cx| InputState::new(window, cx).default_value(settings.base_url.clone())),
            key: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(settings.api_key.clone())
                    .masked(true)
            }),
            language: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(settings.language.clone())
                    .placeholder("Auto-detect, or en / de / …")
            }),
            shortcut: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(settings.toggle_shortcut.clone())
                    .placeholder("Ctrl+Alt+Space")
            }),
            cancel_shortcut: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(settings.cancel_shortcut.clone())
                    .placeholder("Ctrl+Shift+X")
            }),
            language_shortcut: cx.new(|cx| {
                InputState::new(window, cx).default_value(settings.language_shortcut.clone())
            }),
            quick_languages: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(language::quick_languages_text(&settings.quick_languages))
            }),
            validation_error: None,
            last_language: settings.language.clone(),
            last_model: settings.model.clone(),
            language_revision,
            model_selection_revision,
            model,
            device,
            transcript,
            known_models_revision,
            last_transcript: text,
            options: settings,
            retry_confirm: None,
            focus,
            tab: WorkspaceTab::Dictation,
            form_feedback: None,
            form_error: false,
            copied: false,
            _subscriptions: vec![_subscription],
        };
        // Draft edits rerender the save bar without mutating the saved settings.
        for input in [
            &view.url,
            &view.key,
            &view.language,
            &view.shortcut,
            &view.cancel_shortcut,
            &view.language_shortcut,
            &view.quick_languages,
        ] {
            view._subscriptions
                .push(cx.subscribe(input, |this, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.draft_changed(cx);
                    }
                }));
        }
        view._subscriptions
            .push(cx.subscribe(&view.model, |this, _, event, cx| {
                if matches!(event, SelectEvent::Confirm(_)) {
                    this.draft_changed(cx);
                }
            }));
        view._subscriptions
            .push(cx.subscribe(&view.device, |this, _, event, cx| {
                if matches!(event, SelectEvent::Confirm(_)) {
                    this.draft_changed(cx);
                }
            }));
        view
    }
    fn draft(&self, cx: &App) -> anyhow::Result<Settings> {
        let mut settings = self.options.clone();
        // Appearance saves immediately and must never be overwritten by a stale form.
        settings.theme = self.controller.read(cx).settings.theme;
        settings.base_url = self.url.read(cx).value().to_string();
        settings.api_key = self.key.read(cx).unmask_value().to_string();
        settings.language = self.language.read(cx).value().to_string();
        settings.quick_languages =
            language::parse_quick_languages(&self.quick_languages.read(cx).value())?;
        settings.language_shortcut = self.language_shortcut.read(cx).value().to_string();
        settings.toggle_shortcut = self.shortcut.read(cx).value().to_string();
        settings.cancel_shortcut = self.cancel_shortcut.read(cx).value().to_string();
        settings.model = self
            .model
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_default();
        let device = self
            .device
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_default();
        settings.device = if device == "System default" {
            String::new()
        } else {
            device
        };
        Ok(settings)
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let draft = match self.draft(cx) {
            Ok(draft) => {
                self.validation_error = None;
                draft
            }
            Err(error) => {
                self.validation_error = Some(error.to_string());
                self.form_feedback = Some(error.to_string());
                self.form_error = true;
                self.retry_confirm = None;
                cx.notify();
                return false;
            }
        };
        if !self
            .controller
            .update(cx, |c, cx| c.save_settings(draft, cx))
        {
            self.retry_confirm = None;
            self.form_feedback = Some(self.controller.read(cx).notice.clone());
            self.form_error = true;
            cx.notify();
            return false;
        }
        self.form_feedback = Some(self.controller.read(cx).notice.clone());
        self.form_error = self
            .controller
            .read(cx)
            .notice
            .contains("could not be saved");
        self.options = self.controller.read(cx).settings.clone();
        self.language.update(cx, |input, cx| {
            input.set_value(self.options.language.clone(), window, cx)
        });
        self.shortcut.update(cx, |input, cx| {
            input.set_value(self.options.toggle_shortcut.clone(), window, cx)
        });
        self.cancel_shortcut.update(cx, |input, cx| {
            input.set_value(self.options.cancel_shortcut.clone(), window, cx)
        });
        self.language_shortcut.update(cx, |input, cx| {
            input.set_value(self.options.language_shortcut.clone(), window, cx)
        });
        self.quick_languages.update(cx, |input, cx| {
            input.set_value(
                language::quick_languages_text(&self.options.quick_languages),
                window,
                cx,
            )
        });
        cx.notify();
        true
    }
    fn close_or_quit(&mut self, _: &CloseOrQuit, window: &mut Window, cx: &mut Context<Self>) {
        if self.controller.read(cx).settings.quit_on_ctrl_q {
            cx.quit();
        } else {
            self.hide(window, cx);
        }
    }
    fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.controller.update(cx, |c, _| c.window = None);
        window.remove_window();
    }
}
impl DictationView {
    fn change_tab(&mut self, tab: WorkspaceTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        // The old field may no longer be mounted. Keep command handling in the visible view.
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn draft_changed(&mut self, cx: &mut Context<Self>) {
        // Programmatic normalization after Save also emits input changes. Do not
        // erase a persistence warning when those events reflect the applied form.
        if !self.form_error || self.is_dirty(cx) {
            self.form_feedback = None;
            self.form_error = false;
        }
        self.validation_error = None;
        cx.notify();
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.draft(cx)
            .map_or(true, |draft| draft != self.controller.read(cx).settings)
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.is_dirty(cx);
        div()
            .flex()
            .items_center()
            .gap_4()
            .px_5()
            .py_3()
            .flex_shrink_0()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Speaches AI"),
            )
            .child(
                div().flex_1().min_w_0().child(
                    TabBar::new("workspace-tabs")
                        .outline()
                        .small()
                        .selected_index(if self.tab == WorkspaceTab::Dictation {
                            0
                        } else {
                            1
                        })
                        .child(Tab::new().label("Dictation").aria_label("Dictation"))
                        .child(
                            Tab::new()
                                .label(if dirty {
                                    "Settings · unsaved"
                                } else {
                                    "Settings"
                                })
                                .aria_label("Settings"),
                        )
                        .on_click(cx.listener(|this, ix, window, cx| {
                            this.change_tab(
                                if *ix == 0 {
                                    WorkspaceTab::Dictation
                                } else {
                                    WorkspaceTab::Settings
                                },
                                window,
                                cx,
                            );
                        })),
                ),
            )
            .child(
                Button::new("application-menu")
                    .ghost()
                    .small()
                    .label("Menu")
                    .dropdown_menu(|menu, _, _| {
                        menu.menu("Quit application", Box::new(QuitApplication))
                    }),
            )
    }

    fn render_dictation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.controller.read(cx);
        let recording = c.recording.is_some();
        let busy = c.busy;
        let held = c.held_audio.is_some();
        let state = c.state();
        let status = match state {
            Status::Recording(seconds) => {
                format!("Recording · {:02}:{:02}", seconds / 60, seconds % 60)
            }
            Status::Transcribing => "Transcribing".into(),
            Status::Finished => "Transcript ready".into(),
            Status::PasteSkipped => "Paste skipped".into(),
            Status::Error if held => "Audio retained".into(),
            Status::Error => "Needs attention".into(),
            _ => "Ready".into(),
        };
        let status_color = if recording || matches!(state, Status::Error) {
            cx.theme().danger
        } else {
            cx.theme().muted_foreground
        };
        let level = c.level;
        let current_language = c.settings.language.clone();
        let choices = c.settings.quick_languages.clone();
        let selection_error = c.selection_error.clone();
        let notice = c.notice.clone();
        let shortcut = c.settings.toggle_shortcut.clone();
        let device = if c.settings.device.is_empty() {
            "System default".to_owned()
        } else {
            c.settings.device.clone()
        };
        let model = c.settings.model.clone();
        let empty = c.transcript.is_empty();
        let shortcut_error = c.shortcut_error.clone();
        div().flex().flex_col().size_full().min_h_0()
            .child(div().id("recording-controls").flex_shrink_0().flex().flex_col().gap_3().p_5()
                .bg(cx.theme().muted).border_b_1().border_color(cx.theme().border)
                .child(div().flex().flex_wrap().items_center().gap_3()
                    .child(Button::new("record").label(if recording { "Stop and transcribe" } else { "Start recording" })
                        .disabled(busy || held)
                        .on_click(cx.listener(|this, _, _, cx| { this.controller.update(cx, |c, cx| c.toggle(false, cx)); })))
                    .when(recording || busy, |el| el.child(Button::new("cancel").ghost().label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| { this.controller.update(cx, |c, cx| c.cancel(cx)); this.retry_confirm = None; }))))
                    .child(div().id("recording-status").text_sm().text_color(status_color).child(status))
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child(shortcut)))
                .when(recording, |el| el.child(Progress::new("microphone-level").value(level * 100.0)))
                .child(div().flex().flex_wrap().items_center().gap_2()
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Language"))
                    .children(choices.into_iter().map(|code| {
                        let selected = code == current_language;
                        Button::new(SharedString::from(format!("quick-language-{code}"))).small()
                            .label(language::short_label(&code)).selected(selected)
                            .tooltip(language::name(&code).to_owned())
                            .on_click(cx.listener(move |this, _, _, cx| { this.controller.update(cx, |c, cx| { c.select_language(&code, cx); }); }))
                    }))
                    .child(Button::new("quick-preferences").ghost().small().label("More…")
                        .on_click(cx.listener(|this, _, _, cx| { crate::quick_preferences::show(this.controller.clone(), cx); }))))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child(format!("Microphone: {device}")))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child(format!("Model: {model}")))
                .when(recording || busy, |el| el.child(hint("Changes apply to the next recording; the current recording keeps its original settings.", cx)))
                .when_some(selection_error, |el, error| el.child(error_text(error, cx)))
                .when_some(shortcut_error, |el, error| el.child(error_text(error, cx))))
            .when(held && !busy, |el| el.child(self.render_recovery(cx)))
            .child(div().flex().flex_col().flex_1().min_h_0().gap_3().p_5()
                .child(div().flex().items_center().justify_between().gap_3()
                    .child(div().text_lg().font_weight(FontWeight::MEDIUM).child("Latest transcript"))
                    .child(Button::new("copy-now").small().label(if self.copied { "Copied" } else { "Copy" }).disabled(empty)
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(this.controller.read(cx).transcript.clone()));
                            this.copied = true;
                            cx.notify();
                        }))))
                .when(empty, |el| el.child(hint("Start a recording to see your transcript. Audio and transcripts are not saved to disk.", cx)))
                .child(div().flex_1().min_h_0().child(Textarea::new(&self.transcript).aria_label("Latest transcript").readonly(true).h_full()))
                .child(div().id("dictation-notice").text_sm().text_color(cx.theme().muted_foreground).child(notice)))
    }

    fn render_recovery(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let description = self.retry_confirm.as_ref().map(|(_, s)| format!(
            "Upload retained audio to {} using {} · {}? Retry does not copy, paste, or chime.",
            s.base_url, s.model, language::name(&s.language)
        )).unwrap_or_else(|| "Audio is held in memory. Retry uses saved settings; change the destination in Settings first.".into());
        div()
            .id("audio-recovery")
            .flex()
            .flex_col()
            .gap_2()
            .px_5()
            .py_3()
            .flex_shrink_0()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Recording retained"),
            )
            .child(div().text_sm().child(description))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("retry")
                            .label(if self.retry_confirm.is_some() {
                                "Upload audio"
                            } else {
                                "Review retry"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some((revision, snapshot)) = this.retry_confirm.take() {
                                    this.controller.update(cx, |c, cx| {
                                        c.confirm_retry(revision, &snapshot, cx)
                                    });
                                } else {
                                    let c = this.controller.read(cx);
                                    this.retry_confirm =
                                        Some((c.held_revision, c.settings.clone()));
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("discard-audio")
                            .ghost()
                            .label("Discard audio")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.controller.update(cx, |c, cx| c.cancel(cx));
                                this.retry_confirm = None;
                            })),
                    ),
            )
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.controller.read(cx);
        let selected = match c.settings.theme {
            ThemePreference::System => 0,
            ThemePreference::Light => 1,
            ThemePreference::Dark => 2,
        };
        let error = c.theme_error.clone();
        section("Appearance")
            .child(field("Theme", RadioGroup::horizontal("theme-preference")
                .children(["System", "Light", "Dark"]).selected_index(Some(selected))
                .on_change(cx.listener(|this, ix, _, cx| {
                    let preference = match *ix { 1 => ThemePreference::Light, 2 => ThemePreference::Dark, _ => ThemePreference::System };
                    this.controller.update(cx, |c, cx| { c.select_theme(preference, cx); });
                }))))
            .child(hint("Saved immediately. System follows your desktop’s appearance; light is used when no preference is available.", cx))
            .when_some(error, |el, error| el.child(error_text(error, cx)))
    }

    fn render_connection(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.controller.read(cx);
        let changed = self.url.read(cx).value().as_ref() != c.settings.base_url
            || self.key.read(cx).unmask_value().as_ref() != c.settings.api_key;
        let checking = c.checking;
        let connection = c.connection.clone();
        section("Connection")
            .child(field(
                "Backend URL",
                Input::new(&self.url).aria_label("Backend URL"),
            ))
            .child(field(
                "API key",
                Input::new(&self.key).aria_label("API key"),
            ))
            .child(hint("The API key is kept for this session only.", cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("check")
                            .label(if checking {
                                "Checking…"
                            } else {
                                "Refresh models"
                            })
                            .disabled(checking || changed)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.controller.update(cx, |c, cx| c.check(cx));
                            })),
                    )
                    .child(div().text_sm().child(connection)),
            )
            .when(changed, |el| {
                el.child(hint(
                    "Save connection changes before refreshing models.",
                    cx,
                ))
            })
            .child(field(
                "Transcription model",
                Select::new(&self.model)
                    .accessibility_label("Transcription model")
                    .placeholder("Refresh models to discover choices")
                    .menu_max_h(rems(12.)),
            ))
    }

    fn render_audio_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        section("Audio & language")
            .child(field("Microphone", Select::new(&self.device).accessibility_label("Microphone").menu_max_h(rems(12.))))
            .child(field("Language", Input::new(&self.language).aria_label("Language")))
            .child(hint("Leave blank for automatic detection, or enter a code such as en or de. Model language support varies.", cx))
            .child(field("Quick languages", Input::new(&self.quick_languages).aria_label("Quick languages")))
            .child(hint("Comma-separated, for example auto, en, de. This order also controls language cycling.", cx))
            .when_some(self.validation_error.clone(), |el, error| el.child(error_text(error, cx)))
    }

    fn render_shortcuts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.controller.read(cx);
        let status = c.shortcut.clone();
        let error = c.shortcut_error.clone();
        section("Shortcuts")
            .child(field("Toggle recording", Input::new(&self.shortcut).aria_label("Toggle recording")))
            .child(field("Cancel / discard audio", Input::new(&self.cancel_shortcut).aria_label("Cancel / discard audio")))
            .child(field("Cycle quick languages", Input::new(&self.language_shortcut).aria_label("Cycle quick languages")))
            .child(hint("System-wide, including while hidden. Choose distinct combinations with Ctrl, Alt, or Super. Ctrl+Q and paste shortcuts are reserved.", cx))
            .child(hint(status, cx))
            .when_some(error, |el, error| el.child(error_text(error, cx)))
    }

    fn render_behavior(&self, cx: &mut Context<Self>) -> impl IntoElement {
        section("Behavior")
            .child(div().font_weight(FontWeight::MEDIUM).child("After transcription"))
            .child(Checkbox::new("copy").label("Copy to clipboard").checked(self.options.copy)
                .on_change(cx.listener(|this, value, _, cx| { this.options.copy = *value; this.form_feedback = None; cx.notify(); })))
            .child(Checkbox::new("paste").label("Paste if the original window is active").checked(self.options.paste)
                .on_change(cx.listener(|this, value, _, cx| { this.options.paste = *value; this.form_feedback = None; cx.notify(); })))
            .child(hint("Paste replaces the clipboard. If focus changes, paste may be skipped while clipboard contents still change.", cx))
            .child(Checkbox::new("terminal").label("Use Ctrl+Shift+V for terminal paste").checked(self.options.terminal_paste)
                .on_change(cx.listener(|this, value, _, cx| { this.options.terminal_paste = *value; this.form_feedback = None; cx.notify(); })))
            .child(Checkbox::new("chime").label("Play a completion chime").checked(self.options.chime)
                .on_change(cx.listener(|this, value, _, cx| { this.options.chime = *value; this.form_feedback = None; cx.notify(); })))
            .child(div().mt_2().font_weight(FontWeight::MEDIUM).child("Application"))
            .child(Checkbox::new("animate-bar").label("Animate recording status in the bar").checked(self.options.animate_bar)
                .on_change(cx.listener(|this, value, _, cx| { this.options.animate_bar = *value; this.form_feedback = None; cx.notify(); })))
            .child(Checkbox::new("quit-on-ctrl-q").label("Ctrl+Q quits instead of hiding").checked(self.options.quit_on_ctrl_q)
                .on_change(cx.listener(|this, value, _, cx| { this.options.quit_on_ctrl_q = *value; this.form_feedback = None; cx.notify(); })))
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.is_dirty(cx);
        let feedback = self.form_feedback.clone().unwrap_or_else(|| {
            if dirty {
                "Unsaved changes"
            } else {
                "Settings are up to date"
            }
            .into()
        });
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .max_w(rems(44.))
                            .mx_auto()
                            .p_5()
                            .flex()
                            .flex_col()
                            .gap_6()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Settings"),
                                    )
                                    .child(hint(
                                        "Configure your connection and dictation preferences.",
                                        cx,
                                    )),
                            )
                            .child(self.render_appearance(cx))
                            .child(self.render_connection(cx))
                            .child(self.render_audio_settings(cx))
                            .child(self.render_shortcuts(cx))
                            .child(self.render_behavior(cx)),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .w_full()
                            .max_w(rems(44.))
                            .mx_auto()
                            .px_5()
                            .py_3()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(
                                        if self.form_error && self.form_feedback.is_some() {
                                            cx.theme().danger
                                        } else {
                                            cx.theme().muted_foreground
                                        },
                                    )
                                    .child(feedback),
                            )
                            .child(
                                Button::new("save")
                                    .label("Save settings")
                                    .disabled(!dirty && !self.form_error)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.apply(window, cx);
                                        this.retry_confirm = None;
                                    })),
                            ),
                    ),
            )
    }
}

fn section(title: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(div().text_lg().font_weight(FontWeight::MEDIUM).child(title))
}

fn field(label: &'static str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().child(label))
        .child(control)
}

fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

fn error_text(text: String, cx: &App) -> Div {
    div().text_sm().text_color(cx.theme().danger).child(text)
}

impl Render for DictationView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let quit_on_ctrl_q = self.controller.read(cx).settings.quit_on_ctrl_q;
        div()
            .id("dictation-window")
            .key_context("Speaches")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::close_or_quit))
            .on_action(|_: &QuitApplication, _, cx| cx.quit())
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                this.change_tab(WorkspaceTab::Settings, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowDictation, window, cx| {
                this.change_tab(WorkspaceTab::Dictation, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SaveSettings, window, cx| {
                if this.tab == WorkspaceTab::Settings {
                    this.apply(window, cx);
                    this.retry_confirm = None;
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().min_w_0().child(match self.tab {
                WorkspaceTab::Dictation => self.render_dictation(cx).into_any_element(),
                WorkspaceTab::Settings => self.render_settings(cx).into_any_element(),
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .px_5()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(if self.tab == WorkspaceTab::Settings {
                                "Ctrl+S · Save settings"
                            } else {
                                "Ctrl+, · Settings"
                            }),
                    )
                    .child(
                        Button::new("hide")
                            .ghost()
                            .small()
                            .label(if quit_on_ctrl_q {
                                "Hide"
                            } else {
                                "Hide · Ctrl+Q"
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.hide(window, cx))),
                    ),
            )
    }
}
