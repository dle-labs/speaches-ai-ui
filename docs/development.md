# Development setup

Prototype target: Ubuntu 24.04, Regolith 3.4, X11. Rust/Cargo 1.99.0 verified locally; gpui-kit is pinned to 0.7.3 in Cargo.toml and the dependency graph is in Cargo.lock.

## System packages

Conservative Ubuntu baseline, based on [GPUI Kit installation](https://gpui-kit.com/docs/installation.md) plus audio/X11 needs:

```sh
sudo apt update
sudo apt install build-essential pkg-config clang cmake git curl ca-certificates \
  libfontconfig-dev libwayland-dev libwebkit2gtk-4.1-dev \
  libxkbcommon-x11-dev libx11-xcb-dev libxtst-dev \
  libssl-dev libzstd-dev libasound2-dev libvulkan1 \
  mesa-vulkan-drivers vulkan-validationlayers vulkan-tools
```

The user installed these prerequisites. No system packages were installed by the implementation agent. Rust libraries are project dependencies, cached in the user Cargo directory. rustfmt and Clippy were added to the existing user Rust toolchain for verification; neither requires sudo.

## Runtime decisions

- GPUI runs its native event loop with `QuitMode::Explicit`. Closing/hiding removes the visible window; the retained application controller remains alive. A show command creates a fresh view over that controller.
- The main view owns the selected Dictation/Settings tab, retained field entities, and unsaved form draft. Recording, recovery, and saved preferences remain app-owned in Controller. Theme selection persists only the theme and applies immediately; form saves merge the latest saved theme rather than overwriting it with a stale draft.
- `appearance::apply` maps System/Light/Dark to the global GPUI Component theme and refreshes all windows only when the effective mode changes. GPUI receives Linux appearance changes via the XDG desktop portal; the resident loop synchronizes even with no main window. Missing/no preference means Light. The external bar palette is independent.
- An application-owned Tokio runtime (two workers) runs reqwest. Results cross a bounded channel; the GPUI foreground controller consumes them. Request tasks are abortable, and attempt IDs reject late results. Discovery has a separate revision.
- Audio callbacks write to a fixed-capacity buffer without waiting on locks. Encoding currently occurs during stop on the app thread; optimize this into a bounded worker before claiming final V1 responsiveness at the five-minute limit.
- Native X11 integration uses x11rb/XTest. Short queries are synchronous X-server round trips. The adapter fails closed on keyboard mapping changes; restart is needed in this prototype.
- GPUI's X11 clipboard service owns the selection independently of the visible window. Paste preparation is read back before injection; real-client interoperability still requires manual checks.
- Same-user Unix socket clients run without starting GPUI/audio. An advisory lock ensures one server. The socket directory is private and command input/timeouts are bounded.

## Commands

```sh
cargo fmt --all --check
cargo check --locked
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --locked -j 2
./target/debug/speaches-ai
```

Optional isolated real-window checks (no microphone capture or live audio uploads):

```sh
xvfb-run -a python tests/desktop_smoke.py /absolute/artifact-dir/desktop
xvfb-run -a python tests/preferences_smoke.py /absolute/artifact-dir/preferences
xvfb-run -a python tests/theme_smoke.py /absolute/artifact-dir/themes
dbus-run-session -- xvfb-run -a python tests/system_theme_smoke.py /absolute/artifact-dir/system-theme
```

These require Xvfb, xdotool, and ImageMagick; preferences also requires xclip. The portal test additionally requires dbus-python and PyGObject and refuses an already-owned portal name. It supplies a fake portal only inside a fresh D-Bus session, never changes desktop settings, and checks live system changes, fixed overrides, and hidden/quick-only sessions. Other tests use private settings/runtime directories. Screenshot/coordinate fallbacks are documented in the tests; inspect captures after layout changes.

Use a graphical X11 session with a working Vulkan driver. GUI execution under a headless SSH session does not validate real desktop behavior. Read [prototype-status.md](prototype-status.md) for actual test evidence and remaining gaps.
