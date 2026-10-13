# Speaches AI — V1 product specification

Status: agreed core scope plus review-driven safety policies. Proposed convenience defaults are identified separately; they are not additional user requirements.

## Purpose and target

A keyboard-first Linux dictation utility that records speech, sends it to a user-supplied Speaches backend, and delivers the transcript without interrupting the user's current application.

- Product name: **Speaches AI**.
- Target: Ubuntu 24.04 x86_64, Regolith 3.4, i3 4.23, X11.
- Status integration: Regolith's i3xrocks-powered bar; verify actual installed block configuration before implementation.
- UI: Rust, GPUI, and gpui-kit; reuse existing components before creating custom ones.
- Test environment includes three 4K monitors with fractional scaling.
- No backend implementation, local inference engine, or Docker lifecycle management.

## Primary workflow

1. A configurable system-wide shortcut starts recording without showing or focusing the application.
2. Capture the identity of the external active window at recording start.
3. The same shortcut stops recording and submits audio for transcription.
4. Display a transcribing state while awaiting the response.
5. On success, independently perform enabled completion actions: chime, copy, paste.
6. Show a brief completion indication, then return to ready.

A second configurable system-wide shortcut, **Ctrl+Shift+X** by default, cancels capture/transcription or discards retained temporary audio without showing/focusing the app. It also suppresses pending automatic paste but cannot undo output already delivered. Both shortcuts must be distinct; rebinds are transactional and cancel wins over queued toggles.

Only one active recording/transcription job is supported. A toggle during transcription does not start a queue or cancel the request; it reports busy without stealing focus. Ignore keyboard auto-repeat.

### Guarded paste

Immediately before injecting paste input, verify that the external window captured at recording start still exists and is active. Switching away and back is allowed. If a different window is active, skip paste, retain the transcript, and report **“Paste skipped—window changed.”** Never focus or activate a destination to force delivery.

Missing/unknown destinations, destroyed windows, and the application's own window are not valid automatic paste targets. A recording started from the app's button can still produce a transcript and copy it, but must not guess an external destination.

This is a best-effort window-level guard: it cannot detect changing text fields within a window or eliminate every focus race. Input injection also cannot prove the target application accepted the text.

### Output actions

- Chime, copy, and paste are independently configurable.
- Clipboard-based paste may replace the clipboard even if the standalone copy option is off. Explain this in settings; automatic restoration is deferred.
- Check destination and released modifiers before preparing the clipboard, then recheck destination immediately before injection. If paste is skipped before clipboard preparation and copy is off, leave the clipboard unchanged.
- Clipboard ownership is the delivery commit point: if focus changes after preparation, skip injection but report that the transcript may already be on the clipboard. Do not promise transactional restoration.
- Chime and enabled copy still run if paste is skipped.
- No automatic network retries or output replay. Each accepted ordinary attempt delivers at most once; manual retry has the separate policy below.
- Empty/whitespace-only responses do not paste or replace the clipboard; report no speech recognized.
- Preserve a successful transcript if a subsequent output action fails.

## Window and process lifecycle

- Small, resizable utility window: recording control, current state, microphone selection/level, latest transcript, copy/recovery actions, and settings.
- **Ctrl+Q hides by default** while the application continues running.
- Setting **“Ctrl+Q quits application”**, off by default, makes that shortcut fully exit.
- Window close button hides by default; it is independent of the Ctrl+Q setting.
- Explicit **Quit application** always exits, stopping capture and cancelling local work; it must not silently become hide.
- Hiding never cancels recording/transcription.
- Reopening the application or clicking its bar block shows the existing window, not a second instance.
- Proposed conveniences: configurable start hidden and launch at login, both off by default.
- Ctrl+Q is an application-window command, not a global keyboard grab.

## Bar integration

Ship an opt-in i3xrocks block and setup instructions, not a tray-dependent design.

| State | Example text | Meaning |
| --- | --- | --- |
| Ready | MIC | Ready to record |
| Recording | REC 0:12 | Microphone capturing; elapsed time |
| Transcribing | STT … | Backend request in progress |
| Finished | DONE | Brief completion indication |
| Paste skipped | READY / skipped indicator | Transcript available; delivery skipped |
| Error | ERR | Recoverable failure requires attention |
| App absent | OFF | Main process is not running |

Use small distinct state icons (microphone, record circle, processing spinner, check, pause, warning, power), not color alone. Keep elapsed recording time visible and provide a plain-text status alternative. Keep detailed error text in the application, not the bar. Clicking shows the window; it does not start recording. Bar integration must not contact the backend, expose transcripts, or continuously auto-launch the main process.

One application binary may have lightweight status/show client subcommands. This does not require a separately installed audio service or input-injection daemon. Do not overwrite existing user bar configuration.

## Settings

### General
- Configurable global recording toggle and cancel/discard shortcuts; detect conflicts and retain both previous working bindings on failure.
- Start hidden, launch at login, Ctrl+Q quits.

### Audio
- System-default input initially, plus explicit device selection and level feedback.
- Proposed default: a five-minute maximum recording duration, also bounded by a verified memory budget. On either limit, stop capture without automatically uploading; retain the available recording and report the limit with explicit Submit, Save recording…, or Discard actions.
- A limit-held recording blocks further capture until resolved. If a failed recording already exists, use the two-recording decision policy below.
- Start/stop audio cues are deferred; V1 includes only the configured completion chime.

### Backend
- HTTP(S) base URL, optional API key, Check connection.
- Model discovery/selection and supported transcription options, including language.
- Request timeout and understandable authentication, connectivity, TLS, model, and service errors.
- Backend changes apply to subsequent jobs; a job keeps a coherent configuration snapshot.
- Store secrets in the desktop secret service where available; no silent plaintext fallback. Offer session-only credentials if unavailable.
- TLS verification stays enabled. Explain the privacy implications of remote HTTP.
- Disable redirects for audio uploads and mutating management requests. Never forward recorded audio or credentials to a redirected destination. Discovery may follow only explicitly permitted same-origin, non-downgrade redirects.

### Backend management

Assume a Speaches deployment, but determine its documented capabilities and version differences rather than assuming a fixed API. Include a backend/model section exposing relevant supported model operations (for example list, download, load/unload, delete only where the API actually supports them).

Record the verified operation set before implementing this section. Show real progress only if exposed; otherwise show an indeterminate pending state. Require explicit user requests for management changes and confirmations for destructive or resource-intensive operations. Enforce conflicts in the shared command controller, not only by disabling buttons: a pending model mutation blocks conflicting recording/submission/retry, and active dictation blocks conflicting mutations. Revalidate backend/model identity and availability when a confirmation is committed; changing the configured backend invalidates its open management confirmations. Unrelated read operations may proceed. Distinguish unsupported capabilities from unauthorized or temporarily unreachable ones; never probe support through mutations.

Docker start/stop, container upgrades, server filesystem access, and arbitrary server configuration are out of scope.

### Output and privacy
- Independent completion chime, copy, and guarded paste settings.
- V1 preserves returned text without appending a separator. Configurable trailing space/newline is deferred; an appended newline could execute terminal input.
- Latest transcript in session memory; persistent transcript history is not implemented in V1.
- No audio, transcript text, credentials, or authorization headers in routine logs.

## Failure recovery and retention

- Network/backend failure retains the recording temporarily for **Retry**, **Save recording…**, or **Discard**.
- Keep one unresolved failed recording for the process session. One new capture is allowed if the total memory budget permits it. If that capture also fails or reaches a recording limit, retain the original plus one bounded candidate in a `RetentionDecisionPending` state; block additional capture/retry/submission until resolved.
- Report attention in the bar without opening or focusing the window. On reopening, offer Keep original (discard candidate), Keep latest (discard original), or Discard both; allow saving either before deciding. Dismissing the decision preserves both, leaves the block in place, and never silently chooses. After resolution, a limit-held recording still requires explicit Submit or Discard.
- Retry/explicit Submit is available only when no conflicting job or retention decision is active. Show the effective URL/model/options and use current settings only after the user explicitly commits that summary. A changed server destination must be prominently identified and require renewed confirmation; never silently send retained speech to a new server.
- Retry and explicit Submit of retained audio create a fresh immutable configuration snapshot and attempt identity. Their results are displayed for manual copy only: no automatic paste, clipboard replacement, or chime. Success releases that audio; another failure updates that same slot rather than duplicating it. Settings changes while the confirmation summary is open require refreshing and reconfirming the summary.
- Prefer bounded memory for temporary recordings. Budget peak memory across retained audio, live capture, encoding, and HTTP buffers, not duration alone. Reject new capture before exceeding that budget. If disk spill is required, use private application-owned temporary storage and document expiry/crash cleanup before implementing it.
- Successful/cancelled recordings are released after they are no longer needed. Quitting discards transient audio; manual export is the explicit persistence path.
- Device disconnect, recording overflow, malformed responses, timeout, and output failure are visible and recoverable.

## Proposed initial defaults

These are implementation proposals, not previously agreed user preferences. Keep them configurable where listed above.

| Setting | Proposed default |
| --- | --- |
| Completion chime | Off (opt-in); short tone for ordinary successful transcription |
| Automatic copy | Off |
| Automatic paste | Off; user explicitly enables it |
| Audio input | System default |
| Recording duration limit | Five minutes, additionally memory-bounded |
| Start hidden / login startup | Off |

Ctrl+Q hides by default and Ctrl+Shift+X defaults to global cancel are agreed requirements, not proposals. Exact memory and HTTP limits must be measured and documented before capture/network implementation is accepted.

## V1 exclusions

Wayland, streaming transcripts, queues, push-to-talk, voice commands, local speech models, vocabulary management, rich transcript editing, persistent history, automatic clipboard restoration, start/stop cues, configurable trailing separators, and broad packaging support beyond a native Linux installation.

## Delivery criteria

A user can configure a Speaches backend, hide the window, dictate using one shortcut, see live state in Regolith-bar, and receive the selected outputs. Switching to another window reliably takes the skip-paste path in tested scenarios. All recovery, hide/quit, and output settings behave consistently without requiring the UI to remain visible.
