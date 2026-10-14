# Prototype implementation status

This is an end-to-end prototype, not completion of every V1 phase.

## Implemented

- Native gpui-kit window composed from Button, Checkbox, Input, Select, Textarea, and Progress.
- Editable backend URL (default `http://localhost:9791`), session-only API key, STT model discovery, language, microphone selection, and persisted non-secret settings.
- CPAL capture, bounded mono PCM16 WAV, one asynchronous reqwest transcription on an app-owned Tokio runtime, cancellation, and retained audio for explicit retry/discard.
- Configurable native X11 recording toggle (default Ctrl+Alt+Space) and cancel/discard shortcut (default Ctrl+Shift+X). Both bind transactionally; cancel takes priority over queued toggles and uses the existing cancellation path without focusing/showing the window. Focus/lifetime checks, modifier-release guard, Ctrl+V or Ctrl+Shift+V injection, copy, and completion chime.
- App-owned state survives closing/hiding the window. Ctrl+Q hides by default or quits according to settings. Explicit quit exits.
- Same-user single-instance IPC and show/status/toggle/cancel/quit commands, plus preferences, language-next, language/model selection, and read-only selection metadata.
- Quick language buttons and a compact right-click model/language preferences panel using searchable standard Select components. Choices save immediately, synchronize with the main form without saving unrelated drafts, and leave active request snapshots unchanged. Failed persistence retains the old choice.
- Bar language labels with active → next indication; middle-click and configurable Ctrl+Alt+Shift+L cycle favorites (default Auto, English, German). All three global bindings rebind transactionally.
- Colored i3xrocks indicators for ready, recording, transcribing, finished, paste skipped, error, and offline. A persistent worker pulses the recording icon and animates a transcription spinner at 8 fps; idle states stay still. A saved checkbox disables animation. Plain-text and static colored status remain available.
- Opt-in user-local installer/updater adds the desktop launcher, preserves settings/custom blocks, and keeps one rollback snapshot of owned files. Failed startup restores the previous installation; i3 restarts only after successful startup. No automatic desktop changes during development.
- Completion chime shortened from 180 ms to 100 ms, with soft edges. The saved checkbox enables/disables it; new configurations default off, existing saved choices are preserved.

## Verification actually performed

- `cargo check --locked`: passed.
- `cargo build --locked`: passed using Rust 1.99.0 and gpui-kit 0.7.3.
- `cargo test --locked`: **61 passed**, including WAV/downmix/capture limits, URL/auth/multipart/mock responses, redirect non-forwarding, secret exclusion, settings migration/persistence, shortcut parsing, and X11 repeat/lifetime policy tests. One isolated-display test is ignored in the default run.
- `xvfb-run -a cargo test --locked x11::tests::x11_rebind_is_transactional -- --ignored --test-threads=1`: passed. Real X11 grabs verify all three actions, cancel/cycle priority, duplicate rejection, binding swaps, held-key/repeat handling, failed third-binding rollback without leaked grabs, old-binding preservation, and CapsLock/NumLock variants without microphone use.
- `cargo clippy --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --all --check`: passed.
- GPUI window opened/rendered in the actual X11 desktop; application-only screenshot inspected. Backend check displayed seven STT models.
- `tests/desktop_smoke.py` passed under an isolated Xvfb display with private test configuration/runtime storage: window creation, Ctrl+Q hides by default, hidden process status, global cancel without reopening or changing focus, reopening, single-instance show, explicit quit, Ctrl+Q exits when configured, and OFF after exit.
- The desktop smoke test also verifies live/offline `bar` output. All seven glyphs rendered successfully through Pango with the installed FontAwesome font; tests cover icon distinctions, elapsed time, opt-in chime defaults/persistence, and the shorter tone envelope.
- `tests/bar_smoke.py` passed against fake local IPC: colored output, recording pulse, transcription frames, quiet idle/reduced-motion states, left/right/middle click dispatch, language labels, and EOF shutdown. Installed i3xrocks persistent JSON output/click forwarding were separately exercised in an isolated harness. No live bar was restarted.
- `tests/install_local_smoke.py`: **10 passed**, covering first install, repeat/update, retained settings/custom blocks, launcher escaping/validation, preflight failure, and startup-failure rollback. Tests use mocked app/build/i3 commands and isolated directories, not the user's installation.
- `tests/preferences_smoke.py` passed under isolated Xvfb with mock HTTP: compact-only startup, real searchable Select keyboard interactions, model/language persistence, invalid-model recovery, global cycling while hidden, Escape/focus-loss dismissal, same-value quick choice overriding its own draft while preserving unrelated drafts, external selection sync, and save-failure rollback/recovery. Panel/main screenshots inspected. No microphone or live backend inference was used.
- A read-only live probe verified `/health`, `/openapi.json`, and `/v1/models`; see backend-api.md.

No automated check captured the user's microphone or uploaded real speech. Live transcription correctness, real microphone quality, completion sound, paste into browser/editor/terminal, lock-key variations, and multi-monitor layout need user-assisted validation. The user reported the earlier installation working; the new animation and desktop-entry update still need activation through the installer. Mock HTTP and pure X11-policy tests are not proof of those integrations.

## Deliberate prototype limitations

- Main UI remains a scrollable utility form; the small native quick-preferences dialog is separate. It uses managed dialog behavior for reliable X11 focus/floating, not an unsupported anchored popup.
- Global recording/cancel/language shortcut changes apply together on Save settings and survive restart. Validation, duplicates, and conflicts preserve all previous working bindings. Modifier/key names are typed into an existing gpui-kit Input, not captured by a custom control. i3 bindings to `speaches-ai toggle`, `speaches-ai cancel`, and `speaches-ai language-next` are alternatives. Keyboard mapping changes disable native shortcut/paste with feedback until restart.
- Failed/limit-held audio blocks new capture until retry/discard, rather than implementing V1's two-recording resolution UI. Retry requires explicit confirmation bound to the audio revision and saved settings; edits or replacement invalidate that consent. Manual retry has no automatic outputs.
- Device failures discard incomplete audio. Duration/memory limits retain valid bounded audio for explicit submission, never automatic submission.
- Keys are session-only; Secret Service persistence is not yet implemented.
- Model management, recording export, autostart UI, and full UI component integration tests are deferred.
- WAV encoding and native X11 round trips currently occur on the application thread. Move encoding to a bounded worker before final V1 latency acceptance at maximum recording length.
- Paste remains best effort: final focus race, clipboard replacement on late abort, target paste-chord differences, and clipboard-manager behavior after quit are documented rather than hidden.

## Next user check

Run `cargo run --locked`, save/check the backend, choose microphone/model, and try a short recording with automatic paste disabled first. Verify the transcript manually. Then test copy and guarded paste into a disposable text editor before enabling it in routine work. Configure the optional bar block only after the basic flow is working.
