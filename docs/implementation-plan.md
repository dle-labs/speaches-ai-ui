# Speaches AI — V1 implementation plan

Status: original V1 implementation plan, revised after independent scope and technical reviews and subsequent user requests. A working prototype now exists; [prototype-status.md](prototype-status.md) records what is implemented and verified. Uncompleted phases below remain targets, not completion claims.

Product contract: [product-spec.md](product-spec.md). The repository includes the application, installed skills, and their lockfile. See [development.md](development.md) for the verified toolchain/environment and [backend-api.md](backend-api.md) for observed backend capabilities.

## Required skills and implementation rules

Every implementation phase must reference and apply the relevant installed skills:

1. [gpui-kit](../.agents/skills/gpui-kit/SKILL.md): framework setup, state ownership, existing components, async work, actions, and UI integration tests.
   - Read [Coding Guides](../.agents/skills/gpui-kit/references/coding-guides.md) before architecture or state changes.
   - Consult [conventions](../.agents/skills/gpui-kit/references/conventions.md), [recipes](../.agents/skills/gpui-kit/references/recipes.md), and the mechanism-specific `references/gpui/` files before implementing the corresponding feature.
   - The installed skill incorporates core GPUI references; a standalone `gpui` skill is not required.
2. [gpui-kit-design-guides](../.agents/skills/gpui-kit-design-guides/SKILL.md): read the full [Design Guides](../.agents/skills/gpui-kit-design-guides/references/design-guides.md) before new UI work; use its accessibility and design review checklists before declaring a screen complete.
3. [rust-best-practices](../.agents/skills/rust-best-practices/SKILL.md): read relevant chapters before implementation/review, particularly ownership, linting, errors, tests, and thread safety.

**Reuse rule:** use existing gpui-kit components first; compose them to express product behavior. Create a new component only when necessary and document the missing capability, alternatives considered, and tests for the new behavior. Never invent API signatures or copy older GPUI examples without checking current source.

Use `gpui-kit` as the UI dependency, including its GPUI re-exports, rather than mixing independently versioned GPUI/component dependencies. Other non-UI libraries remain appropriate for audio, networking, serialization, and X11 integration.

Keep stateful component entities outside render, keep side effects outside render, use stable domain IDs, preserve keyboard and focus behavior, and use semantic theme tokens/rem-based layout. Separate pure product logic from framework and platform adapters.

## Planned architecture

Start with one Rust package exposing a library for tests and one binary, tentatively `speaches-ai`. Organize modules by capability; do not create a workspace crate for every helper. Split later only when a real independent lifecycle/public boundary justifies it.

```text
src/
  main.rs                      CLI mode selection and application startup
  lib.rs                       Deliberate testable module boundary
  app.rs                       Composition, app-owned services, window lifetime
  dictation/
    mod.rs                     Commands/events and public seam
    state.rs                   Pure job transitions and output policy
    controller.rs              Orchestration and cancellation
    view.rs                    Utility window composition
  audio/                       Device selection, capture, encoding, cues
  backend/                     Speaches HTTP client, capabilities, model operations
  output/                      Clipboard, guarded paste, delivery outcomes
  settings/                    Persistence, validation, secrets, settings UI
  platform/x11/                Hotkey registration, window identity, XTest adapter
  ipc/                         Single instance and bounded local protocol
  bar/                         i3xrocks status formatting and client behavior
assets/                        Application icons and optional licensed sound
integration/i3xrocks/           Opt-in block/configuration template
integration/desktop/            Desktop entry and optional autostart template
tests/                         Mock-server, IPC, lifecycle, adapter integration
```

Names are a proposed layout, not a requirement to create empty modules ahead of need.

### Ownership and concurrency

- One application-owned dictation controller survives hidden/destroyed views. Views observe state and dispatch commands; they do not own capture lifetime.
- Settings, backend capabilities, and transient UI selections have separate owners, not one all-purpose entity.
- Audio callbacks do minimal bounded work. Encoding, network I/O, and blocking X11/IPC work stay off the UI thread.
- Retain task/subscription handles deliberately. Bridge worker results back to GPUI through its supported context mechanisms.
- Each attempt has an identity and immutable settings snapshot. Validate transcription completion against the active attempt and cancellation/shutdown state, not the current settings revision. Changing backend settings does not invalidate a running job. Validate discovery/capability results separately against their backend revision.
- Track transcription outcome separately from each output action. A paste failure is not a failed transcript.
- Use enums for runtime workflow state, typed errors for adapter boundaries, and mockable narrow seams where tests need them. Avoid speculative generic frameworks.

### Workflow state model

```text
Ready -> Recording -> Transcribing -> Delivering -> Finished -> Ready
             |              |              |
             +-> Cancelled  +-> Failed      +-> partial/skipped delivery
```

Finished and error feedback must not discard the latest transcript. Retry storage is separate from the active job. Window visibility is independent of this state machine.

Add explicit limit-held and `RetentionDecisionPending` states. Retain at most one original plus one bounded candidate; block new capture/retry/submission while a replacement decision is pending. Dismissal preserves both without stealing focus. Keep original, Keep latest, or Discard both resolves it; saving either does not silently resolve the decision. A limit-held recording requires explicit submission or discard.

Model mutations are tracked by backend/model identity in the shared controller. Enforce conflicts in both directions for recording, submission, retry, and management; revalidate confirmation commits. UI disabled states reflect this policy but do not implement it.

Commands include toggle, cancel, retry, discard failed recording, show, hide, and quit. GPUI controls and global shortcuts share the same command policy. A second instance forwards show/control requests rather than starting new capture services.

## Phase 0 — Verify dependencies and integration contracts

**Skills:** gpui-kit (coding guides, setup references), Rust best practices (dependency/error/threading decisions).

Tasks:
- Inspect installed Rust/Cargo, compiler/linker, pkg-config, audio development libraries, X11/XTest libraries, Vulkan/graphics support, and available secret service.
- Verify installed Regolith 3.4/i3xrocks configuration discovery paths, block output/click protocol, update mechanism, and behavior across multiple bars.
- Inspect only task-relevant configuration; do not collect broad environment dumps or credentials.
- Check a current compatible gpui-kit release/revision, its actual Linux setup instructions, required Rust version, feature flags, and license. Pin reproducible dependencies and commit Cargo.lock when created.
- Verify a minimal real X11 GPUI window can hide without terminating the process and can later be shown through an external command. Determine supported lifecycle APIs before promising window reuse/recreation details.
- Review official Speaches docs/source/OpenAPI examples. Record URL prefix rules, authentication, transcription upload schema, audio limits/formats, response/error schemas, model listing and management operations, and version differences.
- Create `docs/backend-api.md` with source links and a capability table (operation, method/path, schema, support detection, confirmation policy). Do not assume management routes from OpenAI compatibility alone.
- Evaluate direct Rust HTTP integration first. Candidate tools include reqwest/serde, cpal plus WAV encoding, x11rb with XTest support, and Secret Service integration; verify maintenance and compatibility before selecting versions. Use an SDK only if a maintained compatible Rust SDK is verified.
- Verify HTTP runtime compatibility in an executable spike: async reqwest needs a Tokio runtime, not merely GPUI's background executor. Prefer an app-owned runtime with bounded result channels; document cancellation, worker shutdown, and lifetime independent of windows. If choosing blocking workers instead, prove bounded concurrency and shutdown latency.
- Prove hidden-window HTTP completion, timeout, cancellation, and shutdown before full network integration. Record the selected runtime and ownership model.
- Write `docs/development.md` with actual prerequisites and setup commands. Ask before changing system packages, bar configuration, or autostart.

Exit criteria:
- A tested dependency/lifecycle/runtime approach and documented API/bar contracts, with unresolved gaps stated explicitly. Runtime timeout/cancellation/shutdown and hidden-window completion have executable evidence.
- No requirement to access a live backend yet: use documented contracts and fixtures. Live compatibility verification occurs after the user configures a server.

## Phase 1 — Scaffold, workflow model, settings, and shell

**Skills:** all three; GPUI actions/context/entities/testing references and Rust ownership/errors/testing chapters.

Tasks:
- Create Cargo manifest, lockfile, formatting/lint setup, initial modules and test harness.
- Implement pure transition and completion-policy tests before adapter code.
- Define validated, versioned settings with safe defaults and atomic persistence under XDG config directories. Do not store secrets in ordinary settings.
- Add secret-service integration with a clearly indicated session-only fallback.
- Build the smallest utility window using gpui-kit initialization/root composition from current recipes.
- Implement hide, show, close interception, configurable Ctrl+Q, and explicit quit. Closing overlays must not accidentally invoke application quit.
- Add a same-user single-instance IPC mechanism, preferably a private Unix-domain socket in XDG_RUNTIME_DIR with an atomic ownership/locking strategy. Verify startup races and stale socket cleanup without deleting another live instance's socket.
- Bound request sizes/timeouts, validate commands, restrict permissions, and version the protocol. Do not expose arbitrary shell execution or filesystem paths through IPC.
- Route lightweight CLI client modes before initializing graphics/audio; no running GUI should be required to print OFF status.

Acceptance tests:
- Ctrl+Q hides by default and quits only when enabled; close hides in both settings states.
- Explicit quit releases resources. Hiding leaves the controller alive.
- Simultaneous launch yields one owner. Reopening shows the original window.
- Invalid settings preserve recoverability; secret-service failure does not silently write plaintext credentials.
- Toggle while transcribing returns busy; stale completion cannot mutate the next job.
- Backend A's active transcription completes normally after settings change to B; subsequent capture uses B. A stale A discovery response cannot overwrite B's settings view.

## Phase 2 — Audio capture and global X11 shortcuts

**Skills:** Rust best practices (ownership/thread safety/errors/tests), gpui-kit (async/events), design guides for recording controls.

Tasks:
- Implement system-default and selected microphone capture, bounded buffering, duration limit, elapsed time, and throttled meter updates. Proposed duration default is five minutes; measure and document the aggregate memory ceiling before enabling capture.
- Reaching duration or memory limits stops capture without uploading. Retain available audio, show attention without focusing the window, and require explicit Submit, Save recording…, or Discard. Apply the two-recording decision policy if an older failed recording exists.
- Convert channel/sample formats and encode a backend-supported audio format. Do not assume all devices support one fixed rate.
- Stop/release the microphone before upload. Handle silence, device removal, overflow, and failed capture initialization.
- Register native X11 global toggle, cancel, and quick-language-cycle shortcuts using verified APIs (defaults Ctrl+Alt+Space, Ctrl+Shift+X, and Ctrl+Alt+Shift+L). Handle keymap changes, lock modifiers, auto-repeat, duplicate/conflicting chords, and all-or-nothing rollback when rebinding the set.
- Capture the external active window identity before recording, without activating the application. Track destruction and guard against XID reuse; uncertain identity disables paste, even if the same numeric XID exists. Never rely on window titles as identity.
- Keep manual app-button capture from treating the app itself as a paste target.
- Route the global cancel shortcut through the existing application cancel/discard policy, prioritizing it over queued toggles without stealing focus. Start/stop cues remain deferred; completion chime remains in scope.

Acceptance tests:
- Toggle works while another app is focused, with Caps/Num Lock variations and repeated key events.
- A conflicting binding produces actionable feedback without losing the previous binding.
- Stop/start/device-disconnect paths release audio resources and do not freeze the UI.
- Duration/buffer limits stop without automatic upload and provide explicit recovery. Tests budget peak retained/capture/encoding/upload memory, reject new capture before overflow, and exercise two consecutive failures while hidden, decision dismissal, every resolution, and a limit reached with an existing failed recording.
- Captured/encoded sample fixtures preserve expected duration and format.

## Phase 3 — Speaches connection and transcription

**Skills:** Rust best practices (errors/tests/async ownership), gpui-kit (async tasks and retained state), design guides (validation/pending/error states).

Tasks:
- Implement the verified HTTP adapter with URL normalization, timeout/cancellation, TLS verification, optional authentication, and response-size limits.
- Disable redirects for audio uploads and mutating management requests, including 307/308 body replays. Any discovery redirect allowance must be same-origin and non-downgrade. Do not log audio, transcript, secrets, or sensitive request bodies.
- Add Check connection with separate service reachability/authentication and selected-model availability feedback. Do not run inference merely to check connectivity.
- Discover/select usable STT models and supported language/options; retain saved input if the server is offline.
- Implement one multipart audio submission per attempt, mapping transport, HTTP, schema, model, and no-speech outcomes distinctly.
- Implement the spec's one-failure-plus-one-candidate retention decision, with command-level blocking and non-focus-stealing bar feedback. Export is explicit; dismissing a decision neither discards audio nor permits more capture.
- A cancelled request invalidates local results; do not promise server-side cancellation if unsupported.
- Retry/explicit submission of retained audio shows the effective URL/model/options and commits current settings into a fresh immutable snapshot. Prominently identify a changed destination and require renewed confirmation before sending audio there. Revalidate if settings change while this summary is open.
- Retained-audio submissions display the result for manual copy only: no automatic paste, clipboard replacement, or chime. A retry failure updates its slot rather than creating another retained copy; success releases that audio.

Acceptance tests using a local mock HTTP server:
- Correct URL prefix, authentication, multipart fields/audio, model, and language.
- Timeout, cancellation, 401/403, 404, rate limiting, server failure, malformed/oversized response, and empty transcript.
- Backend edits during a request cannot mix configurations.
- Late replies never paste or overwrite a newer job.
- Retry with changed URL/model/auth requires an explicit effective-configuration commit, does not produce automatic outputs, and follows bounded retention/discard/export policy.
- Mock cross-origin 307/308 and HTTPS downgrade responses: the redirect destination receives neither audio nor credentials. Verify mutating management requests also do not follow redirects.

Milestone: real microphone -> configured backend -> visible transcript, without automatic delivery yet.

## Phase 4 — Clipboard, guarded paste, and chime

**Skills:** Rust best practices (errors/thread safety/tests), gpui-kit (platform capabilities/actions), design guides (partial-success feedback).

Tasks:
- Implement X11 CLIPBOARD ownership using a verified mechanism that remains functional when the GPUI window is hidden. Test Unicode, large transfers, and lifetime behavior.
- Paste through in-process XTest input synthesis with an explicitly documented target paste chord. Default ordinary applications to Ctrl+V; account for terminals commonly needing Ctrl+Shift+V through a small user setting rather than assuming universal Ctrl+V support.
- Recheck the recorded window immediately before injection; abort if unknown, gone, app-owned, or not active. Never force focus.
- Wait safely for user-held shortcut modifiers to release; timeout/skip rather than alter physically held keys. Recheck focus after waiting.
- Establish clipboard readiness before injection. A clipboard failure prevents injection. Validate ownership loss and X11 selection servicing/INCR for large transfers, not just ownership acquisition.
- Use a two-stage guard: validate target/modifiers before clipboard preparation, then validate again immediately before injection. Ownership acquisition is the clipboard commit point; a late focus change skips injection but may leave the transcript on the clipboard. Report that partial outcome; do not implement implicit restoration.
- Track per-action success/failure/skipped outcomes and prevent duplicate delivery.
- Preserve returned text without appending a separator; configurable separators are deferred. If paste is skipped before preparation and copy is disabled, preserve the clipboard. A post-preparation abort may leave it replaced, as explicitly documented.
- Add completion chime, handling audio-output failure independently from transcription success.

Acceptance tests:
- Same window: paste attempted once. Switch away: skipped. Switch away/back: allowed.
- Destroyed/unknown/self destination: skipped. Focus change during modifier wait: skipped.
- Copy/chime/paste combinations behave independently for ordinary attempts. Manual retry has no automatic outputs.
- Deterministically switch focus between clipboard ownership and injection: no paste, clipboard may hold the transcript, and feedback describes this. A pre-preparation skip leaves the clipboard unchanged if copy is off.
- Destroyed/reused XIDs and ownership loss disable injection; clipboard large-transfer tests prove selection servicing.
- Browser/editor/terminal smoke tests with appropriate chord, multiline text, Unicode, and large transcripts.
- Report only injection attempted, not guaranteed delivery; document the unavoidable focus-race limitation.

## Phase 5 — i3xrocks status block

**Skills:** Rust best practices (IPC errors/tests); design guides for concise status text and semantic color.

Tasks:
- Implement `speaches-ai status` and `speaches-ai show` client commands or equivalent names selected with the actual block protocol.
- Publish bounded status snapshots only: state, elapsed time, and outcome category, not audio/transcript/secrets/window titles.
- Choose update behavior after Phase 0 verification: supported persistent/event-driven delivery if available, otherwise bounded inexpensive polling. No backend polling from the bar.
- Handle left-click by showing the main application, right-click by opening compact model/language preferences, and middle-click by cycling configured quick languages. Status polling itself must never launch the application.
- Display selected/active language; show active → next when a preference changes during a recording/request. Keep request snapshots immutable. Share selection commands between main buttons, compact preferences, keyboard, and IPC; do not save unrelated form drafts or silently lose a failed persistence update.
- Provide an opt-in configuration template and installation/removal instructions using verified user-level paths.
- Handle absent/crashed application and IPC version mismatch promptly; tolerate multiple monitor bars without duplicate capture services.
- Use readable fallback text if icon fonts are absent. Keep animation optional and restrained.

Acceptance tests:
- All workflow states are visible in the actual Regolith bar.
- Clicking reopens the existing hidden window.
- Bar stays responsive when app is absent, busy, hung, or restarting.
- No transcript leakage, automatic app restart loop, or duplicate recording process across bars.
- User configuration remains untouched unless installation is explicitly requested.

## Phase 6 — Complete settings and verified backend management

**Skills:** all three; use current component docs and design review checklist.

Prefer these existing components, verifying exact APIs before use:

| Need | Candidate gpui-kit composition |
| --- | --- |
| Recording commands and recovery | Button, DropdownMenu, Kbd |
| Backend URL and secret | Input/InputState with appropriate masking |
| Device/model/language selection | Select or searchable Combobox |
| Independent output choices | Checkbox; Switch only for immediately applied settings |
| Grouped settings | Settings/Form and existing navigation components |
| Current status | Label/Icon, restrained Badge, Spinner |
| Audio level | Existing Progress component if suitable |
| Transcript | Selectable readonly text/input composition |
| Recoverable failure | Inline error and targeted Alert where warranted |
| Destructive backend operation | AlertDialog |
| Model inventory | List; DataTable only if comparison requires columns |

Evaluate the kit's SpeechButton before use, but do not adopt it if its capture/backend ownership conflicts with the app-owned dictation workflow. A standard Button dispatching the shared command is preferable to forcing an incompatible speech abstraction.

Tasks:
- Finish General, Audio, Backend, Models, and Output surfaces with visible labels, validation, pending/disabled states, and keyboard access.
- Expose only verified model-management capabilities from `docs/backend-api.md`; distinguish unsupported, unauthorized, and temporarily unreachable states. Never probe support by mutating backend state.
- Scope confirmations to the named remote backend/model and consequence. Do not mutate/download models merely on connection or selection. Enforce per-backend/model conflicts in the shared command controller in both directions; starting a management operation blocks conflicting dictation/retry and vice versa. Recheck on confirmation commit and invalidate open management confirmations on backend change.
- Invalidate capability responses by backend revision; changes on another server cannot update the current settings view.
- Define minimum/default window dimensions from content and test surplus-space behavior; do not force floating mode or edit i3 rules automatically.
- Persist non-secret settings and opt-in autostart only through explicit user actions.

Acceptance tests:
- Real component UI integration tests exercise input validation, checkbox changes, selection, confirmation cancellation, focus restoration, and loading/error states.
- Full keyboard-only setup and dictation recovery is possible.
- Light/dark themes, larger UI text, narrow/tiled windows, three-monitor movement, and fractional scaling preserve layout and focus visibility.
- No new primitive is introduced without a documented existing-component gap.
- Test management-then-dictation, dictation-then-management, conflicting retry, duplicate mutation, backend change during confirmation, and unrelated reads that should remain available.

## Phase 7 — Hardening and native installation

**Skills:** all three, including both normative review checklists.

Tasks:
- Add desktop entry/icon, native build/install/uninstall instructions, XDG paths, and opt-in login startup.
- Document settings, shortcut conflicts, clipboard replacement, guarded-paste limitations, terminal paste chord, remote HTTP privacy, and recording retention.
- Review shutdown: cancel pending jobs, stop audio, release hotkeys/IPC, ignore late callbacks, and clean temporary artifacts.
- Verify clipboard persistence limitations after quitting and document them rather than assuming a clipboard manager exists.
- Run real user-configured Speaches compatibility checks; record backend version and observed capabilities without credentials.
- Avoid presenting mocked tests as proof of microphone, desktop, or live API integration.

Verification gates once the scaffold exists:

```sh
cargo fmt --check
cargo check --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --locked
cargo build --release --locked
```

Use a documented supported feature matrix if mutually exclusive platform features make an all-features build invalid. Keep mock-server tests deterministic; hardware/X11/live-server tests must be clearly marked and reported separately.

## End-to-end acceptance checklist

- [ ] Configure backend URL/auth, check it, choose STT model, and select microphone.
- [ ] Hide via Ctrl+Q; record/stop via global shortcut while another app stays focused.
- [ ] Recording/transcribing/completion/error are visible in i3xrocks.
- [ ] Each enabled ordinary-attempt output runs once; skipped paste does not suppress enabled copy/chime. Manual retry has no automatic outputs.
- [ ] Pre-preparation paste skips preserve clipboard when copy is off; late aborts skip injection and accurately report possible clipboard replacement.
- [ ] Changed-destination retry requires explicit consent; redirects never forward audio or credentials.
- [ ] Hidden retention collisions and recording limits have bounded, non-interrupting recovery.
- [ ] Management/dictation conflicts are enforced in both directions.
- [ ] Switching away and back allows paste; remaining elsewhere skips it.
- [ ] Opening the Speaches AI window during a request prevents unintended paste into it.
- [ ] Timeout/offline failure retains retry audio without unbounded accumulation.
- [ ] Empty response, cancelled job, and stale reply never inject text.
- [ ] Ctrl+Q setting changes behavior; explicit quit always exits and cleans up.
- [ ] Close hides; launch/bar click shows the single existing instance.
- [ ] Bar integration works on multiple displays and fails gracefully when app is absent.
- [ ] Secrets and transcript/audio content are absent from ordinary settings, IPC status, and logs.
- [ ] Verified backend management works with confirmation and unsupported-state feedback.
- [ ] Tests, real-window design review, and installation instructions are complete.

## Risks and implementation decisions still to verify

The following are technical discovery tasks. Separately, the product spec labels proposed initial output/duration defaults and start-hidden/autostart conveniences; do not misrepresent those as independently agreed user requirements:

- Current gpui-kit Linux lifecycle APIs, graphics/build requirements, and compatible version.
- Actual Regolith/i3xrocks block format and update mechanism on this installation.
- Speaches management operations and capability/version detection; do not invent endpoints.
- Clipboard ownership and input synthesis behavior across real target applications.
- User-chosen conflict-free shortcut and terminal paste chord; both are configuration choices.
- Secret-service availability, microphone backend/device formats, exact aggregate memory limits, and runtime/shutdown compatibility.
- Start/stop cues and configurable trailing separators are deferred, not missing V1 implementation tasks. A separate global cancel shortcut is now in scope per user request.

Build phases incrementally and stop at each acceptance gate. Do not expand V1 into Wayland, a tray subsystem, local inference, streaming, persistent history, or container orchestration.
