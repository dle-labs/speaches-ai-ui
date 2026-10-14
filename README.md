# Speaches AI

An early native GPUI-kit dictation prototype for Ubuntu 24.04 / Regolith 3.4 / X11.
It connects to an existing Speaches backend; no local model engine or Docker service is installed.

## Run

```sh
cargo run --locked
```
I wonder how good we can make this.
The initial build is substantial. On a memory-constrained development session use `cargo build --locked -j 2`, then `./target/debug/speaches-ai`.

Default backend: **http://localhost:9791** (your local SSH forward must be running).
Default model: `deepdml/faster-whisper-large-v3-turbo-ct2`. Use **Save and check connection** to discover available STT models. Model listing does not prove inference readiness.

1. Select the backend/model, language (blank = automatic), and microphone.
2. Select completion actions and **Save settings**. Copy, paste, and the completion chime are initially off. Enable **Enable completion chime (short tone)** for a quiet 100 ms tone, then Save settings. Existing saved chime preferences are preserved.
3. Set **Keyboard shortcuts → Recording toggle**, then **Save settings** (default: **Ctrl+Alt+Space**).
4. Choose a language using the quick buttons, or right-click the bar indicator for compact model/language preferences. **Ctrl+Alt+Shift+L** cycles quick languages without opening a window.
5. Hide the window, focus your destination, and press the recording shortcut to start. Press it again to stop and transcribe. Press **Ctrl+Shift+X** to cancel recording/transcription or discard held audio instead (also configurable).
6. Reopen the app to read or copy the latest transcript.

The visible Record button works too, but does not guess a paste destination. Full-form edits take effect after Save; quick choices apply immediately to future recordings. A running recording/request keeps its original snapshot.

**Ctrl+Q hides**, unless the saved “Ctrl+Q quits application” setting is enabled. The close button always hides. **Quit application** always exits. Hidden windows are recreated from app-owned state; audio/network operations survive hiding.

```sh
./target/debug/speaches-ai show    # launch or reveal existing window
./target/debug/speaches-ai toggle  # remote start/stop (running app required)
./target/debug/speaches-ai preferences   # compact model/language panel only
./target/debug/speaches-ai language-next # cycle configured quick languages
./target/debug/speaches-ai language de   # request German (auto for detection)
./target/debug/speaches-ai model MODEL_ID # request a model from discovery
./target/debug/speaches-ai selection     # read selected model/language as JSON
./target/debug/speaches-ai status  # readable status text
./target/debug/speaches-ai bar     # static colored icon (Pango markup)
# bar-watch is the persistent i3xrocks worker, not a GUI launch command.
./target/debug/speaches-ai cancel  # stop/cancel and discard temporary audio
./target/debug/speaches-ai quit
```

## Safety and prototype boundaries

- Paste is best-effort X11 Ctrl+V, or optional Ctrl+Shift+V for terminals. It checks the original window's lifetime and current focus, never restores focus, and waits briefly for modifiers to be released. Switching away and back is allowed.
- Clipboard-based paste replaces the clipboard. A late focus change skips injection but may leave the transcript on the clipboard. Clipboard restoration is not implemented.
- XTest cannot guarantee delivery or eliminate the final focus race. Do not enable automatic paste into sensitive input fields. The app never appends a newline.
- One recording/request at a time. Failed or duration-limited audio remains in memory; **retry/discard it before starting another recording**. Retry requires an explicit backend/model review and never automatically copies, pastes, or chimes.
- Capture is bounded to five minutes and 32 MiB mono PCM; the request retains one retry copy. The recording limit stops without automatic upload. Device/stream failures discard incomplete audio.
- API keys are **session-only** and excluded from persisted settings. Secure keyring persistence is deferred. No transcript/audio history is written to disk.
- Configuration: `$XDG_CONFIG_HOME/speaches-ai/settings.json` (normally `~/.config/speaches-ai/settings.json`). IPC: private `$XDG_RUNTIME_DIR/speaches-ai/` socket.
- HTTP uploads never follow redirects; HTTPS verification remains enabled. Remote HTTP sends audio/credentials unencrypted—use HTTPS or your trusted local forward.
- All three global shortcuts are configurable and saved: **Ctrl+Alt+Space** toggles recording, **Ctrl+Shift+X** cancels/discards, and **Ctrl+Alt+Shift+L** cycles quick languages. They work while the window is hidden and do not change focus. Cancel aborts the active operation, discards temporary audio, and suppresses pending paste; it cannot undo a paste already delivered. Choose distinct combinations and Save settings. All three rebind together; invalid, duplicate, or taken combinations preserve the previous settings and bindings. Cancel takes priority over queued toggles.
- Supported modifiers: Ctrl/Control, Alt, Shift, Super/Win; keys: letters, digits, Space, F1–F24, Tab, Enter/Return, Escape/Esc, arrows, Home, End, PageUp/PageDown, Insert, Delete, Backspace. At least Ctrl, Alt, or Super is required; Ctrl+Q stays reserved for hide/quit, and Ctrl+V/Ctrl+Shift+V for paste. The key must exist in the current keyboard layout's base group (shifted-only function keys are rejected). Alternatively bind `speaches-ai toggle`, `speaches-ai cancel`, and `speaches-ai language-next` through i3. Keyboard mapping changes currently require restarting the app.
- Deferred: backend model management, persisted history, recording export, autostart UI, full V1 multi-failure recovery, and Wayland support.

## Install or update the app

From the repository, run:

```sh
./scripts/install-local.sh
```

Use the **same command for an update**. No sudo is needed. The script builds and stages everything first, then stops the old instance, replaces the executable, desktop launcher, and icon block, launches the app, and restarts i3. **Finish recording first:** quitting discards temporary audio. Use `--yes` only to explicitly skip the confirmation prompt.

Find **Speaches AI** in your desktop application launcher afterward. Its entry uses the system microphone icon and is installed to `${XDG_DATA_HOME:-$HOME/.local/share}/applications/speaches-ai.desktop`.

Settings and unrelated bar blocks are preserved; the three owned installation files are replaced in place without duplicate launcher entries. On first setup, stock blocks are copied to the new user directory. One previous set of owned files is retained in `${XDG_STATE_HOME:-$HOME/.local/state}/speaches-ai/rollback-latest/`. If the replacement fails to start, the script restores those files and relaunches the previous installation if it was running. Startup diagnostics go to `application.log` in the same state directory. Restarting i3 happens only after successful startup; desktop cache refresh failures produce a warning.

### Manual installation alternative

Nothing is installed into your desktop until you run the installer or deploy the files yourself. Prefer the installer for updates and desktop-entry generation. For a manual executable copy, first quit any running instance, then after building:

```sh
mkdir -p ~/.local/bin
install -m 755 target/debug/speaches-ai ~/.local/bin/speaches-ai
```

The supplied block is `integration/i3xrocks/80_speaches`. The installed Regolith instance uses `~/.config/regolith3/i3xrocks/conf.d` and `/usr/share/i3xrocks/conf.d`.

**Inspect your existing user directory first.** If it does not exist, follow Regolith's setup for retaining your chosen stock blocks (including `01_setup`) before creating it; a partial user directory can change which stock blocks are shown. Do not overwrite existing configuration blindly.

Copy the supplied block into the established user `conf.d` directory. It runs one persistent `bar-watch` process (`interval=persist`, `format=json`, `markup=pango`), rather than spawning a command for every animation frame. Click events arrive as JSON on stdin: **left-click** opens the full app, **right-click** opens compact model/language preferences, and **middle-click** cycles quick languages. Status polling never launches the app; explicit left/right clicks can. Right-click on an offline indicator launches only the compact panel.

### Quick model and language preferences

Right-click opens a small native floating **Quick preferences** panel, not the full app. It uses searchable language/model selectors, displays the current choices, and saves selections immediately. **Refresh models** performs model discovery only; selecting an ID does not explicitly load/unload a backend model. Model language support depends on the backend/model. Escape closes an open selector first, then the panel; Done or switching focus away also dismisses it. A native dialog is used for reliable i3 floating/focus behavior rather than an unsupported anchored X11 popup.

Default quick languages are **Auto → English → German**. Edit **Quick languages (comma-separated)** in the main app, e.g. `en, de, fr`, then Save settings. One to eight languages can be configured; the order controls both buttons and keyboard/middle-click cycling. The compact language selector also offers the full language catalog.

The bar shows the current language (`Auto`, `EN`, `DE`, etc.). Selections apply to the **next recording**; an active recording/request keeps its original model and language. If they differ, the bar shows e.g. `EN → DE` (active → next). Language changes do not change the model. Manual retry uses current settings only after renewed confirmation.

Selections stay synchronized across the panel, bar, keyboard, and main app. Quick choices do not save unrelated unfinished form edits. If a quick choice cannot be saved, the previous selection remains active and an error is shown. CLI selection commands acknowledge queuing; `selection` reports the applied choices and any error.

### Bar appearance

Small icons inherit the bar's font size, with a semantic palette tuned for Regolith's dark bar:

- **Sky-blue microphone:** ready.
- **Pulsing rose record ring + elapsed time:** recording.
- **Lavender spinner:** transcribing.
- **Green circled check:** finished briefly.
- **Amber circled pause:** paste skipped.
- **Rose warning triangle:** error.
- **Muted crossed-out microphone:** app not running.

Busy states animate at eight frames per second. Idle icons do not animate or repeatedly redraw, and idle IPC polling slows to once per second. Disable **Animate recording and transcription in the bar** and Save settings for static indicators. The installed FontAwesome and DejaVu Sans Mono fonts supply the glyphs. `speaches-ai status` remains plain text; `speaches-ai bar` provides a static colored alternative.

Refresh using `i3-msg restart` only when ready. The persistent worker exits on stdin EOF, broken output, or termination signals. When supervising i3xrocks manually, stop its process group rather than sending SIGTERM to only its parent process, which can wait for a still-running worker.

## Development

Rust 1.99 was used for the initial build; the documented GPUI-kit baseline is Rust 1.92+.
System packages are described in [development.md](docs/development.md).

```sh
cargo fmt --all --check
cargo check --locked
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
```

Additional smoke tests take an absolute artifact directory (outside the repository):

```sh
python tests/bar_smoke.py /path/to/test-artifacts/bar
python tests/install_local_smoke.py /path/to/existing-test-artifacts
xvfb-run -a python tests/desktop_smoke.py /path/to/test-artifacts/desktop
xvfb-run -a python tests/preferences_smoke.py /path/to/test-artifacts/preferences
```

The bar test uses fake IPC; installer tests mock build/app/i3 operations and never modify the live desktop. Tests use synthetic samples/mock HTTP servers, not microphone recording or live backend inference. Real desktop/audio validation is separate; see [prototype-status.md](docs/prototype-status.md).

The longer-term [product specification](docs/product-spec.md) and [implementation plan](docs/implementation-plan.md) remain the V1 target, not a claim that every feature is implemented.
