use crate::{
    audio, backend,
    settings::Settings,
    status::Status,
    x11::{Desktop, HotkeyAction, Target},
};
use gpui_kit::*;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) enum Reply {
    Models(u64, Result<Vec<String>, String>),
    Transcript(u64, Result<String, String>),
}
#[derive(Clone, Copy)]
enum Feedback {
    Quiet,
    Finished(Instant),
    PasteSkipped,
    Error,
}

struct PendingPaste {
    target: Target,
    deadline: Instant,
    shift: bool,
}

pub(crate) struct Controller {
    pub(crate) settings: Settings,
    pub(crate) notice: String,
    pub(crate) connection: String,
    pub(crate) shortcut: String,
    pub(crate) shortcut_error: Option<String>,
    pub(crate) transcript: String,
    pub(crate) models: Vec<String>,
    pub(crate) models_revision: u64,
    pub(crate) window: Option<AnyWindowHandle>,
    pub(crate) recording: Option<audio::Recording>,
    pub(crate) busy: bool,
    pub(crate) checking: bool,
    pub(crate) held_audio: Option<Vec<u8>>,
    pub(crate) held_revision: u64,
    pub(crate) level: f32,
    desktop: Option<Desktop>,
    runtime: Arc<tokio::runtime::Runtime>,
    tx: async_channel::Sender<Reply>,
    rx: async_channel::Receiver<Reply>,
    job: Option<tokio::task::JoinHandle<()>>,
    attempt: u64,
    check_revision: u64,
    snapshot: Settings,
    target: Option<Target>,
    manual: bool,
    pending_paste: Option<PendingPaste>,
    feedback: Feedback,
}
impl Controller {
    pub(crate) fn new(runtime: Arc<tokio::runtime::Runtime>) -> Self {
        let (mut settings, notice) = match Settings::load() {
            Ok(s) => (s, "Ready. Check your backend, then record.".into()),
            Err(_) => (
                Settings::default(),
                "Settings could not be loaded; using defaults. Save to replace them.".into(),
            ),
        };
        let (desktop, shortcut) = match Desktop::connect() {
            Ok(d) => {
                let info = match d
                    .register_hotkeys(&settings.toggle_shortcut, &settings.cancel_shortcut)
                {
                    Ok((toggle, cancel)) => {
                        settings.toggle_shortcut = toggle.clone();
                        settings.cancel_shortcut = cancel.clone();
                        format!("{toggle} toggles recording · {cancel} cancels/discards")
                    }
                    Err(e) => {
                        format!(
                            "Shortcuts unavailable: {e}. Change them in settings or bind speaches-ai toggle/cancel in i3."
                        )
                    }
                };
                (Some(d), info)
            }
            Err(e) => (None, format!("X11 unavailable: {e}")),
        };
        let (tx, rx) = async_channel::bounded(16);
        Self {
            snapshot: settings.clone(),
            settings,
            notice,
            connection: "Not checked".into(),
            shortcut,
            shortcut_error: None,
            transcript: String::new(),
            models: Vec::new(),
            models_revision: 0,
            window: None,
            recording: None,
            busy: false,
            checking: false,
            held_audio: None,
            held_revision: 0,
            level: 0.0,
            desktop,
            runtime,
            tx,
            rx,
            job: None,
            attempt: 0,
            check_revision: 0,
            target: None,
            manual: false,
            pending_paste: None,
            feedback: Feedback::Quiet,
        }
    }
    pub(crate) fn save_settings(&mut self, mut next: Settings, cx: &mut Context<Self>) -> bool {
        // Register before changing either settings or persisted data. The X11
        // adapter rolls back new grabs if any key/lock variant conflicts.
        let binding = if let Some(desktop) = self.desktop.as_ref() {
            desktop.register_hotkeys(&next.toggle_shortcut, &next.cancel_shortcut)
        } else if next.toggle_shortcut == self.settings.toggle_shortcut
            && next.cancel_shortcut == self.settings.cancel_shortcut
        {
            Ok((next.toggle_shortcut.clone(), next.cancel_shortcut.clone()))
        } else {
            Err(anyhow::anyhow!(
                "X11 is unavailable. Restart the app before changing its shortcuts"
            ))
        };
        match binding {
            Ok((toggle, cancel)) => {
                next.toggle_shortcut = toggle.clone();
                next.cancel_shortcut = cancel.clone();
                if self.desktop.is_some() {
                    self.shortcut =
                        format!("{toggle} toggles recording · {cancel} cancels/discards");
                }
                self.shortcut_error = None;
            }
            Err(error) => {
                let message = format!(
                    "{error}. Settings were not applied; previous working shortcuts are unchanged."
                );
                self.shortcut_error = Some(message.clone());
                self.notice = message;
                cx.notify();
                return false;
            }
        }
        self.check_revision += 1;
        self.checking = false;
        if self.settings.base_url != next.base_url || self.settings.api_key != next.api_key {
            self.models.clear();
            self.models_revision += 1;
            self.connection = "Not checked".into();
        }
        self.settings = next;
        self.notice = match self.settings.save() {
            Ok(()) => "Settings saved. API key is kept only for this session.".into(),
            Err(_) => "Settings applied for this session, but could not be saved.".into(),
        };
        cx.notify();
        true
    }
    pub(crate) fn check(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.check_revision += 1;
        let revision = self.check_revision;
        self.checking = true;
        self.connection = "Checking…".into();
        let s = self.settings.clone();
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let result = backend::models(&s.base_url, &s.api_key)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(Reply::Models(revision, result)).await;
        });
        cx.notify();
    }
    pub(crate) fn toggle(&mut self, external: bool, cx: &mut Context<Self>) {
        if self.busy || self.pending_paste.is_some() {
            return;
        }
        if self.held_audio.is_some() {
            self.notice =
                "A recording is retained. Retry or discard it before recording again.".into();
        } else if let Some(recording) = self.recording.take() {
            self.level = 0.0;
            match recording.finish() {
                Ok(captured) => {
                    self.held_audio = Some(captured.wav);
                    self.held_revision += 1;
                    if captured.limited {
                        self.notice =
                            "Recording limit reached. Audio held for explicit submission.".into();
                    } else {
                        self.submit(false, cx);
                    }
                }
                Err(e) => {
                    self.feedback = Feedback::Error;
                    self.notice = format!("Recording failed: {e}");
                }
            }
        } else {
            if self.settings.model.trim().is_empty() {
                self.feedback = Feedback::Error;
                self.notice = "Choose a transcription model first.".into();
                cx.notify();
                return;
            }
            self.snapshot = self.settings.clone();
            let own_active = self.window.is_some_and(|h| {
                h.update(cx, |_, w, _| w.is_window_active())
                    .unwrap_or(false)
            });
            self.target = if external && !own_active {
                self.desktop
                    .as_ref()
                    .and_then(|d| d.capture_target().ok().flatten())
            } else {
                None
            };
            let device = if self.snapshot.device.is_empty() {
                None
            } else {
                Some(self.snapshot.device.as_str())
            };
            match audio::Recording::start(device) {
                Ok(r) => {
                    self.recording = Some(r);
                    self.notice = format!(
                        "Listening… {} to transcribe; {} to cancel.",
                        self.settings.toggle_shortcut, self.settings.cancel_shortcut
                    );
                    self.feedback = Feedback::Quiet;
                }
                Err(e) => {
                    self.feedback = Feedback::Error;
                    self.notice = format!("Could not open microphone: {e}");
                }
            }
        }
        cx.notify();
    }
    pub(crate) fn confirm_retry(
        &mut self,
        revision: u64,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) {
        if revision != self.held_revision || settings != &self.settings || self.held_audio.is_none()
        {
            self.notice = "Recording or settings changed. Review the retry again.".into();
            cx.notify();
            return;
        }
        self.submit(true, cx);
    }
    fn submit(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.busy || self.recording.is_some() {
            return;
        }
        let Some(wav) = self.held_audio.as_ref() else {
            return;
        };
        if manual {
            self.snapshot = self.settings.clone();
            self.target = None;
        }
        self.manual = manual;
        self.attempt += 1;
        let id = self.attempt;
        let data = wav.clone(); // Retain one bounded retry copy until this attempt succeeds.
        let s = self.snapshot.clone();
        let tx = self.tx.clone();
        self.busy = true;
        self.feedback = Feedback::Quiet;
        self.notice = "Transcribing…".into();
        self.job = Some(self.runtime.spawn(async move {
            let result = backend::transcribe(&s.base_url, &s.api_key, &s.model, &s.language, data)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(Reply::Transcript(id, result)).await;
        }));
        cx.notify();
    }
    pub(crate) fn cancel(&mut self, cx: &mut Context<Self>) {
        self.attempt += 1;
        self.held_revision += 1;
        if let Some(job) = self.job.take() {
            job.abort();
        }
        self.recording = None;
        self.pending_paste = None;
        self.held_audio = None;
        self.busy = false;
        self.level = 0.0;
        self.feedback = Feedback::Quiet;
        self.notice = "Cancelled. Temporary recording discarded.".into();
        cx.notify();
    }
    pub(crate) fn state(&self) -> Status {
        if let Some(r) = &self.recording {
            Status::Recording(r.elapsed().as_secs())
        } else if self.busy || self.pending_paste.is_some() {
            Status::Transcribing
        } else if self.held_audio.is_some() {
            Status::Error
        } else {
            match self.feedback {
                Feedback::Error => Status::Error,
                Feedback::PasteSkipped => Status::PasteSkipped,
                Feedback::Finished(at) if at.elapsed() < Duration::from_secs(4) => Status::Finished,
                _ => Status::Ready,
            }
        }
    }
    pub(crate) fn status(&self) -> String {
        self.state().text()
    }
    pub(crate) fn tick(&mut self, cx: &mut Context<Self>) {
        if matches!(self.feedback, Feedback::Finished(at) if at.elapsed() >= Duration::from_secs(4))
        {
            self.feedback = Feedback::Quiet;
            cx.notify();
        }
        let hotkey = match self.desktop.as_ref().map(|d| d.poll_hotkey()) {
            Some(Ok(hotkey)) => hotkey,
            Some(Err(_)) => {
                self.desktop = None;
                self.pending_paste = None;
                self.feedback = Feedback::Error;
                self.shortcut = "X11 keyboard mapping or connection changed. Restart Speaches AI to restore shortcut/paste.".into();
                cx.notify();
                None
            }
            None => None,
        };
        if matches!(hotkey, Some(HotkeyAction::Cancel)) {
            self.cancel(cx);
        }
        if matches!(hotkey, Some(HotkeyAction::Toggle)) {
            // A global shortcut invoked inside our own window has no external paste target.
            let own_active = self.window.is_some_and(|h| {
                h.update(cx, |_, w, _| w.is_window_active())
                    .unwrap_or(false)
            });
            self.toggle(!own_active, cx);
        }
        if let Some(r) = &self.recording {
            self.level = r.level();
            let issue = r.error();
            if r.limit_reached() || issue.is_some() {
                if let Some(r) = self.recording.take() {
                    self.held_audio = r.finish().ok().map(|captured| captured.wav);
                    self.held_revision += 1;
                }
                self.level = 0.0;
                self.feedback = Feedback::Error;
                self.notice = if issue.is_some() {
                    "Microphone failed. Incomplete audio was discarded; check your input device."
                } else {
                    "Recording limit reached. Audio held; submit explicitly or discard."
                }
                .into();
            }
            cx.notify();
        }
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::Models(revision, result) if revision == self.check_revision => {
                    self.checking = false;
                    match result {
                        Ok(models) => {
                            self.connection = format!("Connected • {} STT models", models.len());
                            self.models = models;
                            self.models_revision += 1;
                        }
                        Err(e) => self.connection = format!("Connection check failed: {e}"),
                    }
                }
                Reply::Transcript(id, result) if id == self.attempt => {
                    self.busy = false;
                    self.job = None;
                    match result {
                        Ok(text) => {
                            self.held_audio = None;
                            self.transcript = text;
                            if self.transcript.trim().is_empty() {
                                self.feedback = Feedback::Finished(Instant::now());
                                self.notice = "No speech recognized.".into();
                            } else if self.manual {
                                self.notice = "Transcript ready. Copy it manually.".into();
                                self.feedback = Feedback::Finished(Instant::now());
                            } else {
                                self.deliver(cx);
                            }
                        }
                        Err(e) => {
                            self.feedback = Feedback::Error;
                            self.notice =
                                format!("Transcription failed: {e}. Audio retained for retry.")
                        }
                    }
                }
                _ => {}
            }
            cx.notify();
        }
        self.try_paste(cx);
    }
    fn deliver(&mut self, cx: &mut Context<Self>) {
        if self.snapshot.copy {
            cx.write_to_clipboard(ClipboardItem::new_string(self.transcript.clone()));
        }
        if self.snapshot.chime {
            self.runtime.spawn_blocking(|| {
                let _ = audio::chime();
            });
        }
        self.notice = if self.snapshot.copy {
            "Transcript ready and copied."
        } else {
            "Transcript ready."
        }
        .into();
        self.feedback = Feedback::Finished(Instant::now());
        if self.snapshot.paste {
            if let Some(target) = self.target.take() {
                self.pending_paste = Some(PendingPaste {
                    target,
                    deadline: Instant::now() + Duration::from_secs(2),
                    shift: self.snapshot.terminal_paste,
                });
            } else {
                self.feedback = Feedback::PasteSkipped;
                self.notice = "Paste skipped—no external destination. Transcript ready.".into();
            }
        }
    }
    fn try_paste(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.pending_paste.as_ref() else {
            return;
        };
        let Some(d) = &self.desktop else {
            self.pending_paste = None;
            self.feedback = Feedback::Error;
            self.notice = "Paste unavailable—X11 is disconnected. Transcript ready.".into();
            cx.notify();
            return;
        };
        if !d.target_is_active(&p.target).unwrap_or(false) {
            self.pending_paste = None;
            self.feedback = Feedback::PasteSkipped;
            self.notice = "Paste skipped—window changed. Transcript ready.".into();
            cx.notify();
            return;
        }
        if !d.modifiers_released().unwrap_or(false) {
            if Instant::now() >= p.deadline {
                self.pending_paste = None;
                self.feedback = Feedback::PasteSkipped;
                self.notice = "Paste skipped—release modifier keys. Transcript ready.".into();
                cx.notify();
            }
            return;
        }
        // No clipboard restoration: a late focus race can leave this prepared text on the clipboard.
        cx.write_to_clipboard(ClipboardItem::new_string(self.transcript.clone()));
        let ready = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .is_some_and(|text| text == self.transcript);
        let result = if ready {
            d.paste(&p.target, p.shift)
        } else {
            Err(anyhow::anyhow!("Clipboard preparation failed"))
        };
        if result.is_err() {
            self.feedback = if ready && d.target_is_active(&p.target).is_ok_and(|active| !active) {
                Feedback::PasteSkipped
            } else {
                Feedback::Error
            };
        }
        self.notice = match result {
            Ok(()) => "Paste sent to the original window. Transcript available below.".into(),
            Err(_) => if ready {
                "Paste skipped or failed; transcript is on the clipboard."
            } else {
                "Paste skipped—clipboard preparation failed. Use Copy transcript to retry."
            }
            .into(),
        };
        self.pending_paste = None;
        cx.notify();
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
    }
}
