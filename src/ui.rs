use crate::{audio, controller::Controller, settings::Settings};
use gpui_kit::component::{
    ActiveTheme, Disableable, IndexPath,
    button::Button,
    checkbox::Checkbox,
    input::{Input, InputState, Textarea, TextareaState},
    progress::Progress,
    select::{Select, SelectState},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

actions!(speaches, [CloseOrQuit]);

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
            size(px(660.), px(820.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Speaches AI".into()),
            ..Default::default()
        }),
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
    model: Entity<SelectState<Vec<String>>>,
    device: Entity<SelectState<Vec<String>>>,
    transcript: Entity<TextareaState>,
    known_models_revision: u64,
    last_transcript: String,
    options: Settings,
    retry_confirm: Option<(u64, Settings)>,
    focus: FocusHandle,
    _subscription: Subscription,
}
impl DictationView {
    fn new(controller: Entity<Controller>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = controller.read(cx).settings.clone();
        let known_models_revision = controller.read(cx).models_revision;
        let mut models = controller.read(cx).models.clone();
        if !models.contains(&settings.model) {
            models.insert(0, settings.model.clone());
        }
        let chosen = models
            .iter()
            .position(|m| m == &settings.model)
            .map(IndexPath::new);
        let model =
            cx.new(|cx| SelectState::new(models.clone(), chosen, window, cx).searchable(true));
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
            let models = c.models.clone();
            let models_revision = c.models_revision;
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
            if models_revision != this.known_models_revision {
                this.known_models_revision = models_revision;
                let selected = this.model.read(cx).selected_value().cloned();
                this.model.update(cx, |state, cx| {
                    state.set_items(models, window, cx);
                    if let Some(value) = selected {
                        state.set_selected_value(&value, window, cx);
                    }
                });
            }
            if text != this.last_transcript {
                this.last_transcript = text.clone();
                this.transcript
                    .update(cx, |state, cx| state.set_value(text, window, cx));
            }
            cx.notify();
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
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
            model,
            device,
            transcript,
            known_models_revision,
            last_transcript: text,
            options: settings,
            retry_confirm: None,
            focus,
            _subscription,
        }
    }
    fn draft(&self, cx: &App) -> Settings {
        let mut settings = self.options.clone();
        settings.base_url = self.url.read(cx).value().to_string();
        settings.api_key = self.key.read(cx).unmask_value().to_string();
        settings.language = self.language.read(cx).value().to_string();
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
        settings
    }
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let draft = self.draft(cx);
        if !self
            .controller
            .update(cx, |c, cx| c.save_settings(draft, cx))
        {
            self.retry_confirm = None;
            return false;
        }
        self.options = self.controller.read(cx).settings.clone();
        self.shortcut.update(cx, |input, cx| {
            input.set_value(self.options.toggle_shortcut.clone(), window, cx)
        });
        self.cancel_shortcut.update(cx, |input, cx| {
            input.set_value(self.options.cancel_shortcut.clone(), window, cx)
        });
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
impl Render for DictationView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.controller.read(cx);
        let recording = c.recording.is_some();
        let busy = c.busy;
        let held = c.held_audio.is_some();
        let checking = c.checking;
        let status = c.status();
        let notice = c.notice.clone();
        let connection = c.connection.clone();
        let shortcut = c.shortcut.clone();
        let shortcut_error = c.shortcut_error.clone();
        let level = c.level;
        let empty = c.transcript.is_empty();
        let controller = self.controller.clone();
        div().id("dictation-window").key_context("Speaches")
            .track_focus(&self.focus).on_action(cx.listener(Self::close_or_quit))
            .size_full().overflow_y_scroll().p_5().flex().flex_col().gap_3()
            .bg(cx.theme().background).text_color(cx.theme().foreground)
            .child(div().text_xl().child("Speaches AI"))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child("DICTATION PROTOTYPE · X11"))
            .child(div().flex().items_center().gap_3()
                .child(Button::new("record").label(if recording { "Stop and transcribe" } else { "Start recording" })
                    .disabled(busy || held)
                    .on_click(cx.listener(|this, _, _, cx| { this.controller.update(cx, |c, cx| c.toggle(false, cx)); })))
                .child(Button::new("cancel").label("Cancel / discard").disabled(!recording && !busy && !held)
                    .on_click(cx.listener(|this, _, _, cx| { this.controller.update(cx, |c, cx| c.cancel(cx)); this.retry_confirm = None; })))
                .child(status))
            .child(Progress::new("microphone-level").value(level * 100.0))
            .child(div().text_sm().child(shortcut))
            .child(div().text_sm().child(notice))
            .when(held && !busy, |el| el.child(
                div().flex().flex_col().gap_2()
                    .child(self.retry_confirm.as_ref().map(|(_, s)| format!("Upload retained audio to {} using {}? No automatic outputs.", s.base_url, s.model)).unwrap_or_else(|| "Review the destination before uploading retained audio.".into()))
                    .child(Button::new("retry").label(if self.retry_confirm.is_some() { "Confirm upload" } else { "Review retry…" })
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some((revision, snapshot)) = this.retry_confirm.take() {
                                if this.draft(cx) == snapshot {
                                    this.controller.update(cx, |c, cx| c.confirm_retry(revision, &snapshot, cx));
                                } else {
                                    if !this.apply(window, cx) { return; }
                                    let c = this.controller.read(cx);
                                    this.retry_confirm = Some((c.held_revision, c.settings.clone()));
                                }
                            } else {
                                if !this.apply(window, cx) { return; }
                                let c = this.controller.read(cx);
                                this.retry_confirm = Some((c.held_revision, c.settings.clone()));
                            }
                            cx.notify();
                        })))
            ))
            .child(div().text_lg().child("Backend"))
            .child("HTTP(S) URL")
            .child(Input::new(&self.url))
            .child("API key (session only)")
            .child(Input::new(&self.key))
            .child(div().flex().gap_2().items_center()
                .child(Button::new("check").label(if checking { "Checking…" } else { "Save and check connection" }).disabled(checking)
                    .on_click(cx.listener(|this, _, window, cx| { if !this.apply(window, cx) { return; } this.retry_confirm = None; this.controller.update(cx, |c, cx| c.check(cx)); })))
                .child(div().text_sm().child(connection)))
            .child("Transcription model")
            .child(Select::new(&self.model).placeholder("Check connection to discover models"))
            .child("Language")
            .child(Input::new(&self.language))
            .child("Microphone")
            .child(Select::new(&self.device))
            .child(div().text_lg().child("Keyboard shortcuts"))
            .child("Recording toggle (system-wide)")
            .child(Input::new(&self.shortcut))
            .child("Cancel / discard (system-wide)")
            .child(Input::new(&self.cancel_shortcut))
            .child(div().text_sm().text_color(cx.theme().muted_foreground)
                .child("Choose different combinations, then Save settings. Cancel stops recording/transcription and discards temporary audio. Examples: Ctrl+Alt+Space, Super+Shift+R, Ctrl+F9. Ctrl+Q is reserved for hide/quit; Ctrl+V and Ctrl+Shift+V for paste."))
            .when_some(shortcut_error, |el, error| el.child(div().text_sm().text_color(cx.theme().danger).child(error)))
            .child(div().text_lg().child("After transcription"))
            .child(Checkbox::new("copy").label("Copy to clipboard").checked(self.options.copy)
                .on_change(cx.listener(|this, value, _, cx| { this.options.copy = *value; cx.notify(); })))
            .child(Checkbox::new("paste").label("Paste only if the original window is still active").checked(self.options.paste)
                .on_change(cx.listener(|this, value, _, cx| { this.options.paste = *value; cx.notify(); })))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Paste replaces the clipboard. If focus changes after preparation, paste is skipped but clipboard contents may change."))
            .child(Checkbox::new("terminal").label("Use Ctrl+Shift+V for paste (terminals)").checked(self.options.terminal_paste)
                .on_change(cx.listener(|this, value, _, cx| { this.options.terminal_paste = *value; cx.notify(); })))
            .child(Checkbox::new("chime").label("Enable completion chime (short tone)").checked(self.options.chime)
                .on_change(cx.listener(|this, value, _, cx| { this.options.chime = *value; cx.notify(); })))
            .child(Checkbox::new("animate-bar").label("Animate recording and transcription in the bar").checked(self.options.animate_bar)
                .on_change(cx.listener(|this, value, _, cx| { this.options.animate_bar = *value; cx.notify(); })))
            .child(Checkbox::new("quit-on-ctrl-q").label("Ctrl+Q quits application (otherwise hides)").checked(self.options.quit_on_ctrl_q)
                .on_change(cx.listener(|this, value, _, cx| { this.options.quit_on_ctrl_q = *value; cx.notify(); })))
            .child(Button::new("save").label("Save settings").on_click(cx.listener(|this, _, window, cx| { this.apply(window, cx); this.retry_confirm = None; })))
            .child(div().text_lg().child("Latest transcript"))
            .child(Textarea::new(&self.transcript).readonly(true).h_32())
            .child(div().flex().gap_2()
                .child(Button::new("copy-now").label("Copy transcript").disabled(empty)
                    .on_click(move |_, _, cx| { cx.write_to_clipboard(ClipboardItem::new_string(controller.read(cx).transcript.clone())); }))
                .child(Button::new("hide").label("Hide · Ctrl+Q").on_click(cx.listener(|this, _, window, cx| this.hide(window, cx))))
                .child(Button::new("quit").label("Quit application").on_click(|_, _, cx| cx.quit())))
    }
}
