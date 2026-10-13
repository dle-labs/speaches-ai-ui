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

Use a graphical X11 session with a working Vulkan driver. GUI execution under a headless SSH session does not validate real desktop behavior. Read [prototype-status.md](prototype-status.md) for actual test evidence and remaining gaps.
