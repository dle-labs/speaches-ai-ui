//! A small native dialog: X11 has no anchored-popup support, and override-redirect
//! popups cannot reliably receive keyboard focus. Dialog windows also float in i3.
use crate::{controller::Controller, language};
use gpui_kit::base::actions::Cancel;
use gpui_kit::component::{
    ActiveTheme, Disableable, IndexPath,
    button::Button,
    select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

pub(crate) fn init(cx: &mut App) {
    // Reuse Cancel so an open Select consumes Escape before the window does.
    cx.bind_keys([KeyBinding::new("escape", Cancel, Some("QuickPreferences"))]);
}

pub(crate) fn show(controller: Entity<Controller>, cx: &mut App) {
    if let Some(handle) = controller.read(cx).quick_window
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        controller.update(cx, |c, cx| {
            if c.models.is_empty() && !c.checking {
                c.check(cx);
            }
        });
        return;
    }
    let owner = controller.clone();
    let options = WindowOptions {
        kind: WindowKind::Dialog,
        // Native initial bounds are a platform boundary; contents use rem-based layout.
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(440.), px(420.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Speaches AI — Quick preferences".into()),
            ..Default::default()
        }),
        app_id: Some("speaches-ai".into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, move |window, cx| {
        let closed = owner.clone();
        window.on_window_should_close(cx, move |_, cx| {
            closed.update(cx, |c, _| c.quick_window = None);
            true
        });
        cx.new(|cx| QuickPreferences::new(owner, window, cx))
    }) {
        Ok((handle, _)) => controller.update(cx, |c, cx| {
            c.quick_window = Some(handle);
            if c.models.is_empty() && !c.checking {
                c.check(cx);
            }
        }),
        Err(error) => controller.update(cx, |c, cx| {
            c.notice = format!("Could not open quick preferences: {error}");
            cx.notify();
        }),
    }
}

#[derive(Clone)]
struct LanguageChoice(String);
impl SelectItem for LanguageChoice {
    type Value = String;
    fn title(&self) -> SharedString {
        if self.0.is_empty() {
            "Auto-detect".into()
        } else {
            format!("{} ({})", language::name(&self.0), self.0).into()
        }
    }
    fn value(&self) -> &String {
        &self.0
    }
}

struct QuickPreferences {
    controller: Entity<Controller>,
    language: Entity<SelectState<SearchableVec<LanguageChoice>>>,
    model: Entity<SelectState<SearchableVec<String>>>,
    models_revision: u64,
    focus: FocusHandle,
    activated: bool,
    _subscriptions: Vec<Subscription>,
}

impl QuickPreferences {
    fn new(controller: Entity<Controller>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let c = controller.read(cx);
        let language_code = c.settings.language.clone();
        let current_model = c.settings.model.clone();
        let models_revision = c.models_revision;
        let mut models = c.models.clone();
        if !models.contains(&current_model) {
            models.insert(0, current_model.clone());
        }
        let chosen = models
            .iter()
            .position(|model| model == &current_model)
            .map(IndexPath::new);
        let model = cx.new(|cx| {
            SelectState::new(SearchableVec::new(models), chosen, window, cx).searchable(true)
        });
        let mut languages: Vec<_> = language::LANGUAGES
            .iter()
            .map(|(code, _)| LanguageChoice((*code).into()))
            .collect();
        if !languages.iter().any(|item| item.0 == language_code) {
            languages.push(LanguageChoice(language_code.clone()));
        }
        let chosen = languages
            .iter()
            .position(|item| item.0 == language_code)
            .map(IndexPath::new);
        let language = cx.new(|cx| {
            SelectState::new(SearchableVec::new(languages), chosen, window, cx).searchable(true)
        });
        let subscriptions = vec![
            cx.subscribe_in(&language, window, |this, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(code)) = event {
                    this.controller.update(cx, |c, cx| {
                        c.select_language(code, cx);
                    });
                    this.sync(window, cx);
                }
            }),
            cx.subscribe_in(&model, window, |this, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(model)) = event {
                    this.controller.update(cx, |c, cx| {
                        c.select_model(model, cx);
                    });
                    this.sync(window, cx);
                }
            }),
            cx.observe_in(&controller, window, |this, _, window, cx| {
                this.sync(window, cx)
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.activated = true;
                } else if this.activated {
                    this.close(window, cx);
                }
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            controller,
            language,
            model,
            models_revision,
            focus,
            activated: window.is_window_active(),
            _subscriptions: subscriptions,
        }
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let c = self.controller.read(cx);
        let code = c.settings.language.clone();
        let model = c.settings.model.clone();
        let revision = c.models_revision;
        let models = (revision != self.models_revision
            || self.model.read(cx).selected_value() != Some(&model))
        .then(|| c.models.clone());
        // SelectState setters do not emit Confirm (verified in gpui-component 0.7.3).
        // Only user confirmation persists settings, so observer sync cannot loop.
        if self.language.read(cx).selected_value() != Some(&code) {
            self.language.update(cx, |state, cx| {
                state.set_selected_value(&code, window, cx);
                cx.notify();
            });
        }
        if let Some(mut models) = models {
            self.models_revision = revision;
            if !models.contains(&model) {
                models.insert(0, model.clone());
            }
            self.model.update(cx, |state, cx| {
                state.set_items(SearchableVec::new(models), window, cx);
                state.set_selected_value(&model, window, cx);
                cx.notify();
            });
        }
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.controller.update(cx, |c, _| c.quick_window = None);
        window.remove_window();
    }
}

impl Render for QuickPreferences {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.controller.read(cx);
        let checking = c.checking;
        let connection = c.connection.clone();
        let error = c.selection_error.clone();
        div().id("quick-preferences").key_context("QuickPreferences")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Cancel, window, cx| this.close(window, cx)))
            .size_full().overflow_y_scroll()
            .bg(cx.theme().background).text_color(cx.theme().foreground)
            .child(div().p_4().flex().flex_col().gap_3()
                .child(div().text_lg().child("Quick preferences"))
                .child(div().text_sm().text_color(cx.theme().muted_foreground)
                    .child("Changes are saved immediately and apply to the next recording. Model language support varies."))
                .child(div().flex().flex_col().gap_1()
                    .child("Language")
                    .child(Select::new(&self.language).accessibility_label("Language")
                        .search_placeholder("Search languages").menu_max_h(rems(12.))))
                .child(div().flex().flex_col().gap_1()
                    .child("Transcription model")
                    .child(Select::new(&self.model).accessibility_label("Transcription model")
                        .search_placeholder("Search models").menu_max_h(rems(12.))))
                .child(div().text_sm().child(connection))
                .when_some(error, |el, error| el.child(div().text_sm().text_color(cx.theme().danger).child(error)))
                .child(div().flex().items_center().gap_2()
                    .child(Button::new("refresh-models").label(if checking { "Checking…" } else { "Refresh models" })
                        .disabled(checking).on_click(cx.listener(|this, _, _, cx| {
                            this.controller.update(cx, |c, cx| c.check(cx));
                        })))
                    .child(Button::new("done").label("Done").on_click(cx.listener(|this, _, window, cx| this.close(window, cx))))))
    }
}
