//! Small X11-only desktop adapter. Keep it on the UI thread and poll regularly.
//!
//! Event polling never waits for an event or for a key to be released. Queries and
//! checked requests do require ordinary X server round trips; this is not a fully
//! asynchronous transport. Native Wayland windows are outside this adapter's scope.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use anyhow::{Context, Result, bail, ensure};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, GrabMode,
            KEY_PRESS_EVENT, KEY_RELEASE_EVENT, Mapping, ModMask, Window,
        },
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

const XK_CONTROL_L: u32 = 0xffe3;
const XK_SHIFT_L: u32 = 0xffe1;
const XK_ALT_L: u32 = 0xffe9;
const XK_ALT_R: u32 = 0xffea;
const XK_SUPER_L: u32 = 0xffeb;
const XK_SUPER_R: u32 = 0xffec;
const CTRL: u8 = 1;
const ALT: u8 = 2;
const SHIFT: u8 = 4;
const SUPER: u8 = 8;
const XK_NUM_LOCK: u32 = 0xff7f;
const XK_CAPS_LOCK: u32 = 0xffe5;
const XK_SPACE: u32 = 0x20;
const XK_V: u32 = 0x76;

/// An opaque, connection-specific window lifetime, not merely a reusable XID.
#[derive(Clone, Debug)]
pub struct Target {
    window: Window,
    owner: Rc<()>,
    alive: Rc<Cell<bool>>,
}

#[derive(PartialEq, Eq)]
struct Keys {
    control: u8,
    shift: u8,
    v: u8,
    min_keycode: u8,
    width: usize,
    keysyms: Vec<u32>,
    modifier_masks: [u16; 4],
    lock_mask: u16,
    modifiers: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct Shortcut {
    modifiers: u8,
    symbol: u32,
    display: String,
}

/// A physical activation of one configured global shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    Toggle,
    Cancel,
    CycleLanguage,
}

#[derive(PartialEq, Eq)]
struct Binding {
    shortcut: Shortcut,
    keycode: u8,
    mask: u16,
    masks: Vec<u16>,
}

#[derive(Default)]
struct HotkeyState {
    down: bool,
    waiting_for_release: bool,
    release_time: Option<u32>,
    pending: usize,
}

impl HotkeyState {
    fn press(&mut self, time: u32) {
        if self.waiting_for_release {
            return;
        }
        // Core X11 repeat is a release/press pair with the same timestamp. Keep
        // the release across polls because the pair may span socket reads.
        if let Some(released) = self.release_time.take() {
            if released == time {
                return;
            }
            self.down = false;
        }
        if !self.down {
            self.pending = self.pending.saturating_add(1);
            self.down = true;
        }
    }
}

/// Owns the connection, passive hotkey grabs, and watched target lifetimes.
/// Its `Rc` state deliberately prevents moving it to a worker thread.
pub struct Desktop {
    connection: RustConnection,
    root: Window,
    active_window: Atom,
    owner: Rc<()>,
    targets: RefCell<HashMap<Window, Rc<Cell<bool>>>>,
    keys: Keys,
    bindings: RefCell<Option<[Binding; 3]>>,
    mapping_changed: Cell<bool>,
    hotkeys: RefCell<[HotkeyState; 3]>,
}

impl Desktop {
    /// Connects to DISPLAY and requires XTest and a usable core keyboard map.
    pub fn connect() -> Result<Self> {
        let (connection, screen) = x11rb::connect(None).context("Connect to X11 DISPLAY")?;
        let root = connection.setup().roots[screen].root;
        connection
            .xtest_get_version(2, 2)?
            .reply()
            .context("X11 server must support XTest")?;
        let active_window = connection
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")?
            .reply()?
            .atom;
        let keys = resolve_keys(&connection)?;
        Ok(Self {
            connection,
            root,
            active_window,
            owner: Rc::new(()),
            targets: RefCell::new(HashMap::new()),
            keys,
            bindings: RefCell::new(None),
            mapping_changed: Cell::new(false),
            hotkeys: RefCell::new(Default::default()),
        })
    }

    /// Captures the active EWMH client, or returns None if none is advertised.
    pub fn capture_target(&self) -> Result<Option<Target>> {
        self.drain_events()?;
        let Some(window) = self.active_window()? else {
            return Ok(None);
        };
        if !self.targets.borrow().contains_key(&window) {
            self.connection
                .change_window_attributes(
                    window,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
                )?
                .check()
                .context("Watch target window (it may have closed)")?;
            self.targets
                .borrow_mut()
                .insert(window, Rc::new(Cell::new(true)));
        }
        let alive = self.targets.borrow().get(&window).cloned();
        let Some(alive) = alive else {
            return Ok(None);
        };
        let target = Target {
            window,
            owner: Rc::clone(&self.owner),
            alive,
        };
        // Drain again after the round trip, so a queued destruction cannot
        // accidentally validate an old lifetime after the XID has been reused.
        Ok(self.target_is_active(&target)?.then_some(target))
    }

    /// Checks lifetime, EWMH activity, and actual keyboard focus ancestry.
    pub fn target_is_active(&self, target: &Target) -> Result<bool> {
        self.drain_events()?;
        if !target.alive.get() || !Rc::ptr_eq(&target.owner, &self.owner) {
            return Ok(false);
        }
        if self.active_window()? != Some(target.window) {
            return Ok(false);
        }
        let mut focus = self.connection.get_input_focus()?.reply()?.focus;
        let mut focused = false;
        // Toolkits commonly focus a child of the EWMH client window. Refuse
        // unusual/deep trees rather than treating a stale EWMH property as focus.
        for _ in 0..64 {
            if focus == target.window {
                focused = true;
                break;
            }
            if focus <= 1 || focus == self.root {
                break;
            }
            focus = self.connection.query_tree(focus)?.reply()?.parent;
        }
        self.drain_events()?;
        Ok(focused && target.alive.get())
    }

    /// Returns immediately after querying state; never waits for key release.
    /// Lock toggles are allowed, but physically held modifier keys are not.
    pub fn modifiers_released(&self) -> Result<bool> {
        self.drain_events()?;
        let pointer = self.connection.query_pointer(self.root)?.reply()?;
        if u16::from(pointer.mask) & (0xff & !self.keys.lock_mask) != 0 {
            return Ok(false);
        }
        let keys = self.connection.query_keymap()?.reply()?.keys;
        Ok(self.keys.modifiers.iter().all(|&key| !key_down(&keys, key)))
    }

    /// Sends Ctrl+V (or Ctrl+Shift+V) only if the captured target remains focused.
    /// Never focuses a window. Clipboard ownership/content is the caller's job.
    pub fn paste(&self, target: &Target, shift: bool) -> Result<()> {
        ensure!(
            self.modifiers_released()?,
            "Release keyboard modifiers before pasting"
        );
        let state = self.connection.query_keymap()?.reply()?.keys;
        ensure!(
            self.bindings
                .borrow()
                .as_ref()
                .is_none_or(|bindings| bindings
                    .iter()
                    .all(|binding| !key_down(&state, binding.keycode)))
                && !key_down(&state, self.keys.v),
            "Release the hotkey and paste keys before pasting"
        );
        ensure!(
            self.target_is_active(target)?,
            "Paste target is no longer active"
        );

        // Best effort: XTest targets the current focus, so another client/user
        // can change focus after this check. No server/focus grab is attempted.
        let pressed = if shift {
            vec![self.keys.control, self.keys.shift, self.keys.v]
        } else {
            vec![self.keys.control, self.keys.v]
        };
        let mut cookies = Vec::new();
        let mut first_error = None;
        for &key in &pressed {
            match self
                .connection
                .xtest_fake_input(KEY_PRESS_EVENT, key, 0, self.root, 0, 0, 0)
            {
                Ok(cookie) => cookies.push(cookie),
                Err(error) => {
                    first_error = Some(anyhow::Error::from(error));
                    break;
                }
            }
        }
        // Queue every release before checking any server errors. An error on a
        // press must not leave our synthetic modifiers permanently held.
        for &key in pressed.iter().rev() {
            match self
                .connection
                .xtest_fake_input(KEY_RELEASE_EVENT, key, 0, self.root, 0, 0, 0)
            {
                Ok(cookie) => cookies.push(cookie),
                Err(error) => {
                    first_error.get_or_insert_with(|| error.into());
                }
            }
        }
        if let Err(error) = self.connection.flush() {
            first_error.get_or_insert_with(|| error.into());
        }
        for cookie in cookies {
            if let Err(error) = cookie.check() {
                first_error.get_or_insert_with(|| error.into());
            }
        }
        match first_error {
            Some(error) => Err(error.context("Send XTest paste shortcut")),
            None => Ok(()),
        }
    }

    /// Replaces all shortcuts as one transaction, returning canonical names.
    /// Invalid or conflicting bindings leave all previous bindings intact.
    /// Keyboard-map changes still require reconnecting this adapter.
    pub fn register_hotkeys(
        &self,
        toggle: &str,
        cancel: &str,
        cycle_language: &str,
    ) -> Result<(String, String, String)> {
        let next = [
            resolve_binding(&self.keys, parse_shortcut(toggle)?)?,
            resolve_binding(&self.keys, parse_shortcut(cancel)?)?,
            resolve_binding(&self.keys, parse_shortcut(cycle_language)?)?,
        ];
        validate_bindings(&next)?;
        self.drain_events()?;
        let names = (
            next[0].shortcut.display.clone(),
            next[1].shortcut.display.clone(),
            next[2].shortcut.display.clone(),
        );
        if self.bindings.borrow().as_ref() == Some(&next) {
            return Ok(names);
        }
        let old_tuples = self
            .bindings
            .borrow()
            .as_ref()
            .map_or_else(Vec::new, grab_tuples);
        let new_tuples = grab_tuples(&next);
        let mut acquired = Vec::new();
        let mut released = Vec::new();
        let install = (|| -> Result<[u8; 32]> {
            for &(keycode, mask) in &new_tuples {
                if !old_tuples.contains(&(keycode, mask)) {
                    self.grab_key(keycode, mask)?;
                    acquired.push((keycode, mask));
                }
            }
            // Bound pre-install events before committing, including mapping
            // changes. A held new trigger must release before it can activate.
            let state = self.connection.query_keymap()?.reply()?.keys;
            self.drain_events()?;
            for &(keycode, mask) in &old_tuples {
                if !new_tuples.contains(&(keycode, mask)) {
                    self.connection
                        .ungrab_key(keycode, self.root, ModMask::from(mask))?
                        .check()?;
                    released.push((keycode, mask));
                }
            }
            Ok(state)
        })();
        let state = match install {
            Ok(state) => state,
            Err(error) => {
                // Usually nothing was released (e.g. a grab conflict). On a
                // transport failure restoration can only be best effort.
                for (keycode, mask) in released {
                    let _ = self.grab_key(keycode, mask);
                }
                for (keycode, mask) in acquired {
                    if let Ok(cookie) =
                        self.connection
                            .ungrab_key(keycode, self.root, ModMask::from(mask))
                    {
                        let _ = cookie.check();
                    }
                }
                return Err(error.context(format!(
                    "Cannot register {}, {}, and {}; another application may own a shortcut",
                    names.0, names.1, names.2
                )));
            }
        };
        let mut bindings = self.bindings.borrow_mut();
        let mut hotkeys = self.hotkeys.borrow_mut();
        for (ix, binding) in next.iter().enumerate() {
            if bindings.as_ref().is_none_or(|old| old[ix] != *binding) {
                hotkeys[ix] = HotkeyState {
                    waiting_for_release: key_down(&state, binding.keycode),
                    ..HotkeyState::default()
                };
            }
        }
        *bindings = Some(next);
        Ok(names)
    }

    fn grab_key(&self, keycode: u8, mask: u16) -> Result<()> {
        self.connection
            .grab_key(
                false,
                self.root,
                ModMask::from(mask),
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )?
            .check()?;
        Ok(())
    }

    /// Nonblocking drain; cancel discards queued toggles, but preserves cycles.
    /// Language cycles take priority over toggles so new captures use that language.
    /// Also invalidates destroyed targets, even when no action is pending.
    pub fn poll_hotkey(&self) -> Result<Option<HotkeyAction>> {
        self.drain_events()?;
        Ok(take_hotkey(&mut self.hotkeys.borrow_mut()))
    }

    fn active_window(&self) -> Result<Option<Window>> {
        let reply = self
            .connection
            .get_property(false, self.root, self.active_window, AtomEnum::WINDOW, 0, 1)?
            .reply()?;
        if reply.type_ != u32::from(AtomEnum::WINDOW) || reply.format != 32 {
            return Ok(None);
        }
        Ok(reply
            .value32()
            .and_then(|mut values| values.next())
            .filter(|&window| window > 1 && window != self.root))
    }

    fn drain_events(&self) -> Result<()> {
        while let Some(event) = self.connection.poll_for_event()? {
            match event {
                Event::DestroyNotify(event) if event.response_type & 0x80 == 0 => {
                    if let Some(alive) = self.targets.borrow_mut().remove(&event.window) {
                        alive.set(false);
                    }
                }
                Event::KeyPress(event) if event.response_type & 0x80 == 0 => {
                    if let Some(bindings) = self.bindings.borrow().as_ref() {
                        let mut hotkeys = self.hotkeys.borrow_mut();
                        for (binding, hotkey) in bindings.iter().zip(hotkeys.iter_mut()) {
                            if event.detail == binding.keycode
                                && u16::from(event.state) & (0xff & !self.keys.lock_mask)
                                    == binding.mask
                            {
                                hotkey.press(event.time);
                            }
                        }
                    }
                }
                Event::KeyRelease(event) if event.response_type & 0x80 == 0 => {
                    if let Some(bindings) = self.bindings.borrow().as_ref() {
                        let mut hotkeys = self.hotkeys.borrow_mut();
                        // Release applies to every action on this physical key,
                        // regardless of modifiers at release time.
                        for (binding, hotkey) in bindings.iter().zip(hotkeys.iter_mut()) {
                            if event.detail == binding.keycode {
                                hotkey.waiting_for_release = false;
                                hotkey.release_time = Some(event.time);
                            }
                        }
                    }
                }
                Event::MappingNotify(event)
                    if event.request == Mapping::KEYBOARD || event.request == Mapping::MODIFIER =>
                {
                    // Fail closed rather than synthesize stale keycodes or keep
                    // silently using an obsolete global shortcut mapping.
                    // X servers may announce an unchanged map during initial
                    // client/XTest setup. Only invalidate a genuinely new map.
                    match resolve_keys(&self.connection) {
                        Ok(keys) if keys == self.keys => {}
                        Ok(_) => self.mapping_changed.set(true),
                        Err(error) => {
                            // An unusable replacement map must stay failed
                            // closed on subsequent polls, not just this one.
                            self.mapping_changed.set(true);
                            return Err(error);
                        }
                    }
                }
                Event::Error(error) => bail!("X11 protocol error: {error:?}"),
                _ => {}
            }
        }
        // A trigger held before its passive grab was installed might release
        // without sending us any event. Query once per poll, never wait for it.
        if self
            .hotkeys
            .borrow()
            .iter()
            .any(|hotkey| hotkey.waiting_for_release)
        {
            let keys = self.connection.query_keymap()?.reply()?.keys;
            if let Some(bindings) = self.bindings.borrow().as_ref() {
                for (binding, hotkey) in bindings.iter().zip(self.hotkeys.borrow_mut().iter_mut()) {
                    if !key_down(&keys, binding.keycode) {
                        hotkey.waiting_for_release = false;
                    }
                }
            }
        }
        ensure!(
            !self.mapping_changed.get(),
            "Keyboard mapping changed; reconnect the desktop adapter"
        );
        Ok(())
    }
}

fn take_hotkey(hotkeys: &mut [HotkeyState; 3]) -> Option<HotkeyAction> {
    if hotkeys[1].pending > 0 {
        hotkeys[0].pending = 0;
        hotkeys[1].pending -= 1;
        Some(HotkeyAction::Cancel)
    } else if hotkeys[2].pending > 0 {
        hotkeys[2].pending -= 1;
        Some(HotkeyAction::CycleLanguage)
    } else if hotkeys[0].pending > 0 {
        hotkeys[0].pending -= 1;
        Some(HotkeyAction::Toggle)
    } else {
        None
    }
}

fn validate_bindings(bindings: &[Binding; 3]) -> Result<()> {
    for (index, left) in bindings.iter().enumerate() {
        for right in &bindings[index + 1..] {
            ensure!(
                !bindings_overlap(left, right),
                "Toggle, cancel, and cycle-language shortcuts must use distinct keyboard chords"
            );
        }
    }
    Ok(())
}

fn bindings_overlap(left: &Binding, right: &Binding) -> bool {
    left.keycode == right.keycode && left.masks.iter().any(|mask| right.masks.contains(mask))
}

fn grab_tuples(bindings: &[Binding; 3]) -> Vec<(u8, u16)> {
    bindings
        .iter()
        .flat_map(|binding| binding.masks.iter().map(|&mask| (binding.keycode, mask)))
        .collect()
}

fn resolve_binding(keys: &Keys, shortcut: Shortcut) -> Result<Binding> {
    let keycode = find_base_key(&keys.keysyms, keys.width, keys.min_keycode, shortcut.symbol)
        .with_context(|| format!("Keyboard has no base key for {}", shortcut.display))?;
    let mut mask = 0;
    for (ix, bit) in [CTRL, ALT, SHIFT, SUPER].into_iter().enumerate() {
        if shortcut.modifiers & bit != 0 {
            let slot = keys.modifier_masks[ix];
            ensure!(
                slot.count_ones() == 1,
                "Requested modifier must map to exactly one X11 slot"
            );
            ensure!(
                slot & (mask | keys.lock_mask) == 0,
                "Requested modifier slots overlap"
            );
            mask |= slot;
        }
    }
    Ok(Binding {
        shortcut,
        keycode,
        mask,
        masks: grab_masks(mask, keys.lock_mask),
    })
}

fn resolve_keys(connection: &RustConnection) -> Result<Keys> {
    let setup = connection.setup();
    let count = u8::try_from(u16::from(setup.max_keycode) - u16::from(setup.min_keycode) + 1)
        .context("X11 keycode range is too large")?;
    let mapping = connection
        .get_keyboard_mapping(setup.min_keycode, count)?
        .reply()?;
    let width = usize::from(mapping.keysyms_per_keycode);
    ensure!(width > 0, "X11 keyboard mapping is empty");
    let find = |symbol| find_base_key(&mapping.keysyms, width, setup.min_keycode, symbol);
    let required =
        |symbol, name| find(symbol).with_context(|| format!("Keyboard has no {name} key"));
    let modifiers = connection.get_modifier_mapping()?.reply()?.keycodes;
    let per_modifier = modifiers.len() / 8;
    ensure!(per_modifier > 0, "X11 modifier mapping is empty");
    let mask_for = |key: Option<u8>| -> u16 {
        key.map_or(0, |key| {
            modifiers
                .chunks(per_modifier)
                .enumerate()
                .filter(|(_, keys)| keys.contains(&key))
                .fold(0, |mask, (index, _)| mask | (1 << index))
        })
    };
    let alt_mask = mask_for(find(XK_ALT_L)) | mask_for(find(XK_ALT_R));
    let super_mask = mask_for(find(XK_SUPER_L)) | mask_for(find(XK_SUPER_R));
    let control = required(XK_CONTROL_L, "Control_L")?;
    let shift = required(XK_SHIFT_L, "Shift_L")?;
    ensure!(
        mask_for(Some(control)) == u16::from(ModMask::CONTROL),
        "Control_L has a nonstandard modifier mapping"
    );
    ensure!(
        mask_for(Some(shift)) == u16::from(ModMask::SHIFT),
        "Shift_L has a nonstandard modifier mapping"
    );
    let lock_mask =
        u16::from(ModMask::LOCK) | mask_for(find(XK_NUM_LOCK)) | mask_for(find(XK_CAPS_LOCK));

    Ok(Keys {
        control,
        shift,
        v: required(XK_V, "v")?,
        min_keycode: setup.min_keycode,
        width,
        modifier_masks: [
            mask_for(Some(control)),
            alt_mask,
            mask_for(Some(shift)),
            super_mask,
        ],
        keysyms: mapping.keysyms,
        lock_mask,
        modifiers: modifiers.into_iter().filter(|&key| key != 0).collect(),
    })
}

// Only the first, unshifted group is supported. Searching every column can
// silently bind F13 to Shift+F1's physical key, or a letter to an AltGr symbol.
fn find_base_key(keysyms: &[u32], width: usize, minimum: u8, symbol: u32) -> Option<u8> {
    if width == 0 {
        return None;
    }
    keysyms
        .chunks_exact(width)
        .position(|symbols| symbols[0] == symbol)
        .and_then(|index| u8::try_from(index).ok())
        .and_then(|index| minimum.checked_add(index))
}

fn parse_shortcut(input: &str) -> Result<Shortcut> {
    let mut modifiers = 0;
    let mut key = None;
    for token in input.split('+').map(str::trim) {
        let lower = token.to_ascii_lowercase();
        let modifier = match lower.as_str() {
            "ctrl" | "control" => Some(CTRL),
            "alt" => Some(ALT),
            "shift" => Some(SHIFT),
            "super" | "win" => Some(SUPER),
            _ => None,
        };
        if let Some(modifier) = modifier {
            ensure!(modifiers & modifier == 0, "Duplicate modifier: {token}");
            modifiers |= modifier;
            continue;
        }
        ensure!(key.is_none(), "Shortcut must contain exactly one key");
        let (symbol, name) = match lower.as_str() {
            "space" => (XK_SPACE, "Space".to_owned()),
            "tab" => (0xff09, "Tab".to_owned()),
            "return" | "enter" => (0xff0d, "Enter".to_owned()),
            "escape" | "esc" => (0xff1b, "Escape".to_owned()),
            "left" => (0xff51, "Left".to_owned()),
            "up" => (0xff52, "Up".to_owned()),
            "right" => (0xff53, "Right".to_owned()),
            "down" => (0xff54, "Down".to_owned()),
            "home" => (0xff50, "Home".to_owned()),
            "end" => (0xff57, "End".to_owned()),
            "pageup" => (0xff55, "PageUp".to_owned()),
            "pagedown" => (0xff56, "PageDown".to_owned()),
            "insert" => (0xff63, "Insert".to_owned()),
            "delete" => (0xffff, "Delete".to_owned()),
            "backspace" => (0xff08, "Backspace".to_owned()),
            _ if lower.len() == 1 && lower.as_bytes()[0].is_ascii_alphanumeric() => {
                (u32::from(lower.as_bytes()[0]), lower.to_ascii_uppercase())
            }
            _ => {
                let number = lower
                    .strip_prefix('f')
                    .and_then(|s| s.parse::<u32>().ok())
                    .filter(|n| (1..=24).contains(n) && lower == format!("f{n}"));
                let number = number.with_context(|| format!("Unknown shortcut key: {token}"))?;
                (0xffbd + number, format!("F{number}"))
            }
        };
        key = Some((symbol, name));
    }
    ensure!(
        modifiers & (CTRL | ALT | SUPER) != 0,
        "Shortcut needs Ctrl, Alt, or Super"
    );
    let (symbol, name) = key.context("Shortcut is missing a key")?;
    ensure!(
        modifiers != CTRL || symbol != u32::from(b'q'),
        "Ctrl+Q is reserved for hide/quit"
    );
    ensure!(
        symbol != XK_V || (modifiers != CTRL && modifiers != (CTRL | SHIFT)),
        "Ctrl+V and Ctrl+Shift+V are reserved for paste"
    );
    let mut parts = Vec::new();
    for (bit, name) in [
        (CTRL, "Ctrl"),
        (ALT, "Alt"),
        (SHIFT, "Shift"),
        (SUPER, "Super"),
    ] {
        if modifiers & bit != 0 {
            parts.push(name);
        }
    }
    parts.push(&name);
    Ok(Shortcut {
        modifiers,
        symbol,
        display: parts.join("+"),
    })
}

fn grab_masks(base: u16, locks: u16) -> Vec<u16> {
    (0..=255)
        .filter(|mask| mask & !locks == 0)
        .map(|mask| base | mask)
        .collect()
}

fn key_down(keys: &[u8; 32], key: u8) -> bool {
    keys[usize::from(key / 8)] & (1 << (key % 8)) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_aliases_are_canonical() {
        for (input, expected) in [
            (" alt + CONTROL + space ", "Ctrl+Alt+Space"),
            ("Win+shift+a", "Shift+Super+A"),
            ("ctrl+Return", "Ctrl+Enter"),
            ("super+esc", "Super+Escape"),
            ("ctrl+9", "Ctrl+9"),
            ("ctrl+shift+q", "Ctrl+Shift+Q"),
            ("alt+shift+control+l", "Ctrl+Alt+Shift+L"),
        ] {
            assert_eq!(parse_shortcut(input).unwrap().display, expected);
        }
    }

    #[test]
    fn shortcut_accepts_supported_keys() {
        for key in [
            "Tab",
            "Enter",
            "Escape",
            "Left",
            "Right",
            "Up",
            "Down",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "Insert",
            "Delete",
            "Backspace",
            "Space",
        ] {
            assert_eq!(
                parse_shortcut(&format!("Ctrl+{key}")).unwrap().display,
                format!("Ctrl+{key}")
            );
        }
        for number in 1..=24 {
            assert_eq!(
                parse_shortcut(&format!("Alt+f{number}")).unwrap().symbol,
                0xffbd + number
            );
        }
        for key in b'a'..=b'z' {
            assert!(parse_shortcut(&format!("Super+{}", char::from(key))).is_ok());
        }
    }

    #[test]
    fn shortcut_rejects_invalid_or_reserved_input() {
        for input in [
            "",
            "Space",
            "Shift+A",
            "Ctrl",
            "Ctrl+Alt",
            "Ctrl+Control+A",
            "Win+Super+A",
            "Ctrl++A",
            "Ctrl+A+",
            "Ctrl+A+B",
            "Ctrl+Q",
            "control+q",
            "Ctrl+V",
            "Ctrl+Shift+V",
            "Ctrl+F0",
            "Ctrl+F25",
            "Ctrl+F01",
            "Ctrl+F+1",
            "Ctrl+é",
            "Ctrl+Unknown",
        ] {
            assert!(parse_shortcut(input).is_err(), "accepted {input:?}");
        }
    }

    #[test]
    fn base_key_lookup_does_not_use_shifted_function_keys_or_overflow() {
        assert_eq!(
            find_base_key(&[0xffbe, 0xffca, 0xffca, 0], 2, 8, 0xffca),
            Some(9)
        );
        assert_eq!(find_base_key(&[0xffbe, 0xffca], 2, 8, 0xffca), None);
        assert_eq!(find_base_key(&[1, 2], 1, 255, 2), None);
        assert_eq!(find_base_key(&[1], 0, 8, 1), None);
    }

    // Run only on an isolated server, e.g.:
    // xvfb-run -a cargo test x11::tests::x11_rebind_is_transactional -- --ignored --test-threads=1
    #[test]
    #[ignore = "Requires isolated Xvfb with XTest; injects keyboard input"]
    fn x11_rebind_is_transactional() -> Result<()> {
        let desktop = Desktop::connect()?;
        let (other, _) = x11rb::connect(None)?;
        let root = desktop.root;
        let input = |kind, code| -> Result<()> {
            other
                .xtest_fake_input(kind, code, 2, root, 0, 0, 0)?
                .check()?;
            desktop.connection.get_input_focus()?.reply()?;
            Ok(())
        };
        let trigger = |key: u8, modifiers: &[u8]| -> Result<()> {
            for &code in modifiers {
                input(KEY_PRESS_EVENT, code)?;
            }
            input(KEY_PRESS_EVENT, key)?;
            input(KEY_RELEASE_EVENT, key)?;
            for &code in modifiers.iter().rev() {
                input(KEY_RELEASE_EVENT, code)?;
            }
            Ok(())
        };
        let key_for = |symbol| {
            find_base_key(
                &desktop.keys.keysyms,
                desktop.keys.width,
                desktop.keys.min_keycode,
                symbol,
            )
            .context("Test key unavailable")
        };
        let space = key_for(XK_SPACE)?;
        let b = key_for(u32::from(b'b'))?;
        let c = key_for(u32::from(b'c'))?;
        let x = key_for(u32::from(b'x'))?;
        let d = key_for(u32::from(b'd'))?;
        let l = key_for(u32::from(b'l'))?;
        let alt = key_for(XK_ALT_L)?;
        let ctrl = [desktop.keys.control];
        let ctrl_alt = [desktop.keys.control, alt];
        let ctrl_shift = [desktop.keys.control, desktop.keys.shift];
        let ctrl_alt_shift = [desktop.keys.control, alt, desktop.keys.shift];
        let cycle = "Ctrl+Alt+Shift+L";
        assert_eq!(
            desktop.register_hotkeys("Control+Alt+Space", "Ctrl+Shift+X", cycle)?,
            ("Ctrl+Alt+Space".into(), "Ctrl+Shift+X".into(), cycle.into())
        );
        trigger(l, &ctrl_alt_shift)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::CycleLanguage));
        trigger(space, &ctrl_alt)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Toggle));
        trigger(x, &ctrl_shift)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Cancel));
        assert_eq!(desktop.poll_hotkey()?, None);

        // Unchanged registration retains the held action's repeat state.
        for &modifier in &ctrl_shift {
            input(KEY_PRESS_EVENT, modifier)?;
        }
        input(KEY_PRESS_EVENT, x)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Cancel));
        desktop.register_hotkeys("ctrl+alt+space", "control+shift+x", cycle)?;
        input(KEY_PRESS_EVENT, x)?;
        assert_eq!(desktop.poll_hotkey()?, None);
        input(KEY_RELEASE_EVENT, x)?;
        for &modifier in ctrl_shift.iter().rev() {
            input(KEY_RELEASE_EVENT, modifier)?;
        }
        desktop.poll_hotkey()?;

        // Cancel discards toggles but preserves every queued language cycle.
        trigger(space, &ctrl_alt)?;
        trigger(l, &ctrl_alt_shift)?;
        trigger(l, &ctrl_alt_shift)?;
        trigger(x, &ctrl_shift)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Cancel));
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::CycleLanguage));
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::CycleLanguage));
        assert_eq!(desktop.poll_hotkey()?, None);
        trigger(space, &ctrl_alt)?;
        trigger(l, &ctrl_alt_shift)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::CycleLanguage));
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Toggle));
        assert!(
            desktop
                .register_hotkeys("Ctrl+Space", "Control+space", cycle)
                .is_err()
        );
        assert!(desktop.register_hotkeys("Ctrl+Q", "Ctrl+B", cycle).is_err());
        assert!(
            desktop
                .register_hotkeys("Ctrl+B", "Ctrl+X", "Control+b")
                .is_err()
        );
        assert!(
            desktop
                .register_hotkeys("Ctrl+B", "Ctrl+X", "Control+x")
                .is_err()
        );
        assert!(
            desktop
                .register_hotkeys("Ctrl+B", "Ctrl+X", "Ctrl+Q")
                .is_err()
        );
        trigger(l, &ctrl_alt_shift)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::CycleLanguage));
        trigger(space, &ctrl_alt)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Toggle));
        trigger(x, &ctrl_shift)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Cancel));

        desktop.register_hotkeys("Ctrl+B", "Ctrl+Space", cycle)?;
        trigger(b, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Toggle));
        trigger(space, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Cancel));
        desktop.register_hotkeys("Ctrl+Space", "Ctrl+B", cycle)?;
        trigger(b, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Cancel));
        trigger(space, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Toggle));
        // A three-way rotation reuses the union of grabs without ungrabbing it.
        desktop.register_hotkeys(cycle, "Ctrl+Space", "Ctrl+B")?;
        trigger(l, &ctrl_alt_shift)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Toggle));
        trigger(space, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Cancel));
        trigger(b, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::CycleLanguage));
        desktop.register_hotkeys("Ctrl+B", "Ctrl+X", cycle)?;

        let masks = grab_masks(u16::from(ModMask::CONTROL), desktop.keys.lock_mask);
        // All old lock variants must now be free to another connection.
        for &mask in &masks {
            other
                .grab_key(
                    false,
                    root,
                    ModMask::from(mask),
                    space,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )?
                .check()?;
            other
                .ungrab_key(space, root, ModMask::from(mask))?
                .check()?;
        }
        // Conflict on the LAST variant: earlier successful grabs must roll back.
        let conflict = *masks.last().unwrap();
        other
            .grab_key(
                false,
                root,
                ModMask::from(conflict),
                c,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )?
            .check()?;
        // First binding D succeeds; second C fails on its last lock variant.
        assert!(desktop.register_hotkeys("Ctrl+D", "Ctrl+C", cycle).is_err());
        // Third binding C fails after BOTH earlier bindings acquired new grabs.
        assert!(
            desktop
                .register_hotkeys("Ctrl+D", "Ctrl+Space", "Ctrl+C")
                .is_err()
        );
        trigger(l, &ctrl_alt_shift)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::CycleLanguage));
        trigger(b, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Toggle));
        trigger(x, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Cancel));
        for key in [d, space] {
            for &mask in &masks {
                other
                    .grab_key(
                        false,
                        root,
                        ModMask::from(mask),
                        key,
                        GrabMode::ASYNC,
                        GrabMode::ASYNC,
                    )?
                    .check()?;
                other.ungrab_key(key, root, ModMask::from(mask))?.check()?;
            }
        }
        for &mask in masks.iter().filter(|&&mask| mask != conflict) {
            other
                .grab_key(
                    false,
                    root,
                    ModMask::from(mask),
                    c,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )?
                .check()?;
            other.ungrab_key(c, root, ModMask::from(mask))?.check()?;
        }
        other
            .ungrab_key(c, root, ModMask::from(conflict))?
            .check()?;
        assert!(desktop.register_hotkeys("Ctrl+B", "Ctrl+Q", cycle).is_err());
        trigger(b, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Toggle));

        // Exercise CapsLock, NumLock, and their combination where available.
        let locks: Vec<_> = [XK_CAPS_LOCK, XK_NUM_LOCK]
            .into_iter()
            .filter_map(|symbol| key_for(symbol).ok())
            .collect();
        let toggle_lock = |code| -> Result<()> {
            other
                .xtest_fake_input(KEY_PRESS_EVENT, code, 2, root, 0, 0, 0)?
                .check()?;
            other
                .xtest_fake_input(KEY_RELEASE_EVENT, code, 2, root, 0, 0, 0)?
                .check()?;
            Ok(())
        };
        for &lock in locks.iter().chain(&locks) {
            toggle_lock(lock)?;
            trigger(b, &ctrl)?;
            assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Toggle));
        }

        // Same physical trigger, different masks: releases update all states.
        desktop.register_hotkeys("Ctrl+B", "Ctrl+Shift+B", "Ctrl+Alt+B")?;
        trigger(b, &ctrl_alt)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::CycleLanguage));
        trigger(b, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Toggle));
        trigger(b, &ctrl_shift)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Cancel));
        trigger(b, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Toggle));

        input(KEY_PRESS_EVENT, x)?;
        desktop.register_hotkeys("Ctrl+B", "Ctrl+X", cycle)?;
        assert_eq!(desktop.poll_hotkey()?, None);
        input(KEY_RELEASE_EVENT, x)?;
        assert_eq!(desktop.poll_hotkey()?, None);
        trigger(x, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::Cancel));

        input(KEY_PRESS_EVENT, c)?;
        desktop.register_hotkeys("Ctrl+B", "Ctrl+X", "Ctrl+C")?;
        assert_eq!(desktop.poll_hotkey()?, None);
        input(KEY_RELEASE_EVENT, c)?;
        assert_eq!(desktop.poll_hotkey()?, None);
        trigger(c, &ctrl)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::CycleLanguage));

        input(KEY_PRESS_EVENT, desktop.keys.control)?;
        input(KEY_PRESS_EVENT, c)?;
        assert_eq!(desktop.poll_hotkey()?, Some(HotkeyAction::CycleLanguage));
        desktop.register_hotkeys("Ctrl+B", "Ctrl+X", "Control+c")?;
        input(KEY_PRESS_EVENT, c)?;
        assert_eq!(desktop.poll_hotkey()?, None);
        input(KEY_RELEASE_EVENT, c)?;
        input(KEY_RELEASE_EVENT, desktop.keys.control)?;
        assert_eq!(desktop.poll_hotkey()?, None);
        Ok(())
    }

    #[test]
    fn cancel_discards_all_queued_toggles_without_resetting_held_state() {
        let mut hotkeys: [HotkeyState; 3] = Default::default();
        hotkeys[0].press(10);
        hotkeys[0].release_time = Some(20);
        hotkeys[0].press(30);
        hotkeys[1].press(40);
        assert_eq!(take_hotkey(&mut hotkeys), Some(HotkeyAction::Cancel));
        hotkeys[0].press(50);
        hotkeys[1].release_time = Some(60);
        hotkeys[1].press(60);
        assert_eq!(take_hotkey(&mut hotkeys), None);
        hotkeys[1].release_time = Some(70);
        hotkeys[1].press(80);
        assert_eq!(take_hotkey(&mut hotkeys), Some(HotkeyAction::Cancel));
    }

    #[test]
    fn cancel_preserves_cycles_and_cycles_precede_toggle() {
        let mut hotkeys: [HotkeyState; 3] = Default::default();
        hotkeys[0].press(10);
        hotkeys[1].press(20);
        hotkeys[2].press(30);
        hotkeys[2].release_time = Some(40);
        hotkeys[2].press(50);
        assert_eq!(take_hotkey(&mut hotkeys), Some(HotkeyAction::Cancel));
        assert_eq!(take_hotkey(&mut hotkeys), Some(HotkeyAction::CycleLanguage));
        assert_eq!(take_hotkey(&mut hotkeys), Some(HotkeyAction::CycleLanguage));
        assert_eq!(take_hotkey(&mut hotkeys), None);

        hotkeys[0].release_time = Some(60);
        hotkeys[0].press(70);
        hotkeys[2].release_time = Some(80);
        hotkeys[2].press(90);
        assert_eq!(take_hotkey(&mut hotkeys), Some(HotkeyAction::CycleLanguage));
        assert_eq!(take_hotkey(&mut hotkeys), Some(HotkeyAction::Toggle));
        assert_eq!(take_hotkey(&mut hotkeys), None);
    }

    #[test]
    fn cycle_repeat_and_preheld_trigger_do_not_queue_extra_actions() {
        let mut hotkeys: [HotkeyState; 3] = Default::default();
        hotkeys[2].waiting_for_release = true;
        hotkeys[2].press(10);
        assert_eq!(take_hotkey(&mut hotkeys), None);
        hotkeys[2].waiting_for_release = false;
        hotkeys[2].release_time = Some(20);
        hotkeys[2].press(30);
        assert_eq!(take_hotkey(&mut hotkeys), Some(HotkeyAction::CycleLanguage));
        hotkeys[2].press(40);
        hotkeys[2].release_time = Some(50);
        hotkeys[2].press(50);
        assert_eq!(take_hotkey(&mut hotkeys), None);
        hotkeys[2].release_time = Some(60);
        hotkeys[2].press(70);
        assert_eq!(take_hotkey(&mut hotkeys), Some(HotkeyAction::CycleLanguage));
    }

    #[test]
    fn validation_rejects_overlap_in_every_pair_including_third_binding() {
        let binding = |keycode| Binding {
            shortcut: parse_shortcut("Ctrl+A").unwrap(),
            keycode,
            mask: 4,
            masks: vec![4, 6],
        };
        for keycodes in [[8, 8, 9], [8, 9, 8], [8, 9, 9]] {
            assert!(validate_bindings(&keycodes.map(binding)).is_err());
        }
        let distinct = [8, 9, 10].map(binding);
        assert!(validate_bindings(&distinct).is_ok());
        assert_eq!(
            grab_tuples(&distinct),
            vec![(8, 4), (8, 6), (9, 4), (9, 6), (10, 4), (10, 6)]
        );
    }

    #[test]
    fn resolved_modifier_aliases_overlap_even_with_different_names() {
        let keys = Keys {
            control: 1,
            shift: 2,
            v: 3,
            min_keycode: 8,
            width: 1,
            keysyms: vec![XK_SPACE],
            modifier_masks: [4, 8, 1, 8],
            lock_mask: 2,
            modifiers: vec![],
        };
        let alt = resolve_binding(&keys, parse_shortcut("Alt+Space").unwrap()).unwrap();
        let super_key = resolve_binding(&keys, parse_shortcut("Super+Space").unwrap()).unwrap();
        assert!(bindings_overlap(&alt, &super_key));
        let ctrl = resolve_binding(&keys, parse_shortcut("Ctrl+Space").unwrap()).unwrap();
        assert!(!bindings_overlap(&alt, &ctrl));
    }

    #[test]
    fn preheld_trigger_is_suppressed_until_release() {
        let mut hotkey = HotkeyState {
            waiting_for_release: true,
            ..Default::default()
        };
        hotkey.press(10);
        assert_eq!(hotkey.pending, 0);
        hotkey.waiting_for_release = false;
        hotkey.release_time = Some(20);
        hotkey.press(30);
        assert_eq!(hotkey.pending, 1);
    }

    #[test]
    fn grabs_cover_all_lock_combinations() {
        assert_eq!(grab_masks(12, 18), vec![12, 14, 28, 30]);
    }

    #[test]
    fn grabs_without_num_lock_are_not_duplicated() {
        assert_eq!(grab_masks(12, 2), vec![12, 14]);
    }

    #[test]
    fn destruction_invalidates_cloned_target() {
        let target = Target {
            window: 42,
            owner: Rc::new(()),
            alive: Rc::new(Cell::new(true)),
        };
        let captured = target.clone();
        target.alive.set(false);
        assert!(!captured.alive.get());
    }

    #[test]
    fn reused_xid_does_not_revive_old_target() {
        let old = Rc::new(Cell::new(false));
        let mut targets = HashMap::new();
        targets.insert(42_u32, Rc::new(Cell::new(true)));
        assert!(!Rc::ptr_eq(&old, &targets[&42]));
    }

    #[test]
    fn repeat_pair_across_polls_does_not_toggle_again() {
        let mut hotkey = HotkeyState::default();
        hotkey.press(10);
        hotkey.release_time = Some(20);
        hotkey.press(20);
        assert_eq!(hotkey.pending, 1);
    }

    #[test]
    fn detectable_repeat_without_release_does_not_toggle_again() {
        let mut hotkey = HotkeyState::default();
        hotkey.press(10);
        hotkey.press(20);
        assert_eq!(hotkey.pending, 1);
    }

    #[test]
    fn new_physical_press_toggles_again() {
        let mut hotkey = HotkeyState::default();
        hotkey.press(10);
        hotkey.release_time = Some(20);
        hotkey.press(30);
        assert_eq!(hotkey.pending, 2);
    }
}
