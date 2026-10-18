# Striper: AI handoff

Read [PLAN.md](../PLAN.md) first; it is the spec, and its "Implementation status" table is the current scoreboard. This file is the short version for the next engineer or agent.

## Current development policy

- **Windows-first product:** Striper is primarily for Dad on Windows 11. Prioritize his dictation, voice correction, accessibility, setup and everyday applications. Linux is Ben's development desktop and a supported platform, not the primary acceptance target.
- **Windows test route:** Ben may use WinBoat to run a Windows 11 guest. Validate the Windows installer, adapters and actual guest applications there; final checks still need Dad's own microphone, GPU and applications. Do not count a Linux VM or Linux test pass as Windows acceptance.
- Installer packaging and full Nix builds are manual. Do not launch repeated builds while iterating. Main Branch Build defaults to Linux and offers Windows/both when explicitly requested.
- Use local core/platform/listener tests and frontend checks for iteration, then batch a testable set of changes. See [LINUX_TESTING.md](LINUX_TESTING.md).
- Vocabulary now includes a read-only compatibility panel and a copyable diagnostic report. It reports helper detection separately from working device/compositor permissions.
- Idle CPU (2026-10-18). Ben saw ~13% CPU after ending a hands-free session; with “Listen for correct that” off it was 0%. Measured here: Vosk decoding costs only ~4% of one core and its bundled OpenBLAS is single-threaded (an `OPENBLAS_NUM_THREADS` fix was reverted). The cause was never pinned down from code, so the listener was made cheap and short-lived instead:
  - **When it holds the mic:** only for 60 s after a push-to-talk dictation or a voice correction (`ListenerArm`, `striper_listener::holds_mic`), during a 30 s follow-up conversation (a pending follow-up lapses with its 60 s window), or while a gated utterance is open (at most 20 s past the window, `UTTERANCE_HOLD_MS`, so a TV or call can't keep it running). Hands-free commits never arm it, and starting or ending a hands-free session, a cancel or turning the setting off disarm it at once (“off means off”). Otherwise phase `standby`, mic closed.
  - **Speech gate:** the selected VAD (Silero/Earshot, Earshot if Silero fails) runs per frame; Vosk only gets audio while `SpeechGate` is open (60 ms onset, 500 ms pre-roll replayed at onset, 1.2 s hangover, then `Detector::end_utterance` as a backstop to Vosk's own endpoint). `crates/striper-listener/tests/gated_wake.rs` (ignored; needs `VOSK_LIB`, `VOSK_MODEL`, `WAKE_CLIPS`) checks that synthetic “correct that” clips still wake and silence is never decoded.
  - **Focus checks** (kdotool on Wayland) only when Vosk's text contains “correct” or a wake/conversation is in progress; follow-ups check focus against their own window, so they also work after live-text (UIA) corrections. A new dictation drops a pending follow-up, so a stale one can no longer turn the next utterance into an instruction.
  - **Diagnostics:** once a minute while the mic was held (or when the whole app used >5% of a core), the log reports listener-thread and whole-process CPU (Linux/macOS; counters only on Windows).
  - **Dev builds:** per-frame audio crates are optimized (`[profile.dev.package.*]` in `src-tauri/Cargo.toml`); the `pulseaudio` crate alone made an open microphone cost ~20% of a core unoptimized.
  - Edit-time focus checks stay uncached and fail when a previously known window becomes unobservable. Real idle CPU still needs confirming on Ben's machine.

## Model picker and compute visibility

- Model setup, the Models catalog, and the downloaded-model footer picker support search and alphabetical model lists. The small featured onboarding recommendations retain curation; matching full-catalog results are visible while searching. Provider model menus are alphabetical too.
- Parakeet Unified EN 0.6B is the first English model to try on Ben's RTX 4090. The current unified GGUF engine supports GPU execution; Linux/Windows x64 compile Vulkan, not CUDA. Do not repeat the old CPU-only Parakeet description or promise measured speed on either user's hardware.
- Every catalog model runs through transcribe-cpp, so there is no per-model CPU/GPU split; the footer picker shows the active model's real placement (On GPU · backend / On CPU) from the loaded worker, and a Recommended tag. Codex's uniform per-row "CPU / GPU" label was removed as noise.
- Plasma 6 Wayland on Ubuntu releases with ydotool 0.1.x: `scripts/linux/setup-plasma-wayland.sh` installs helpers, a udev uaccess rule for /dev/uinput, a `striper-ydotoold` systemd user service (0.1.x must run as the user: fixed /tmp/.ydotool_socket, mode 0600) and kdotool v0.3.0 in /usr/local/bin. kdotool build verified in CI container; the script itself still needs Ben's machine.
- Shortcuts on Wayland: upstream's Linux default is the Tauri global-shortcut backend, which only sees keys while an XWayland window is focused (Ben's "Ctrl+Alt+Space does nothing"). Startup now switches Wayland sessions to handy-keys (evdev, needs the `input` group); handy-keys falls back to non-blocking detection without /dev/uinput, and a dead listener now fails init instead of silently registering nothing.
- Hardware menus and detected GPU summaries include the backend. The model picker/Models page report the worker's actual bound CPU/GPU, backend and device, separately from saved preference and detected devices. Refresh follows model lifecycle events and app focus; no GPU execution has been verified on Ben's desktop merely by adding this status.

## What Striper is

A fork of [Handy](https://github.com/cjpais/Handy) (Tauri 2: Rust backend, React frontend) for a Dragon user on Windows 11 with limited hand mobility. Two lanes share one microphone and one Whisper model:

- **Lane A, dictation.** Hold Ctrl+Space, speak, release. Handy's pipeline, plus learned spellings applied before the text is pasted. Never touches the LLM.
- **Lane B, correction.** Enable the local CPU listener in Vocabulary settings, then say “correct that” and what to change about recent dictation. Follow-ups remain voice-only for 30 seconds. Ctrl+Alt+Space (or `striper --toggle-correction`) is the fallback. Corrections are interpreted by the configured AI only (Ben, 2026-10-17: no regex parsing of edits); only undo, done and cancel are recognised locally so they work instantly and offline. The result is a small list of validated `EditOperation`s that Rust executes and verifies. The LLM never gets keystrokes, shell or desktop access.

## October 17 continuation: voice activation

- `crates/striper-listener/` dynamically loads Vosk 0.3.45 on a dedicated CPU thread. It uses a small keyword grammar (with an unknown-speech alternative) to propose **final** “correct that” candidates. The existing selected recognizer independently confirms the exact wake prefix before any session opens or parser/LLM runs. Partial hypotheses only show feedback; they cannot activate an edit. This is an alternative to training a custom openWakeWord model; no Python runtime is needed.
- `src-tauri/src/striper/listener.rs` owns the shared-mic idle consumer, bounded audio queue, 20-second utterance limit, two-second pre-roll, 30-second follow-up session, cancellation generations and target checks. Once an instruction ends, the existing selected recognizer transcribes its audio. Background audio is never written to history/WAV or uploaded.
- `listener_install.rs` downloads pinned native/model ZIPs on an explicit setup click, verifies SHA-256, rejects unsafe paths/symlinks and oversized archives, stages and validates the engine before installing. Supported initially: Windows x64 and Linux x64. Other architectures keep the shortcut path.
- `MicrophoneSink` in `managers/audio.rs` selects idle listening or normal capture on the **same** recorder. PTT synchronously preempts the listener. The coordinator accepts voice work only while idle and never queues it behind another dictation.
- Cancellation is checked after LLM interpretation and before application. `KeyboardTarget` checks its original foreground identity before each editing action.
- `striper-core::migration::import_handy` imports settings and completed local model files on a fresh non-portable Striper installation, preserving the old install and existing Striper stores. Updater and listener opt-ins are not inherited.

### User setup

Open **Vocabulary → Voice correction**, click **Download local listener files**, then enable **Listen for “correct that”**. After dictation, release Ctrl+Space and say “correct that, change Thursday to Friday”; either say the whole instruction in one breath or pause after the wake phrase and wait for “Listening for a correction” before continuing. Continue with “actually Saturday”, “undo that”, and “done”. Focus must match known recent dictation. Unknown focus falls back to the shortcut. Disable/re-enable after a microphone error to retry.

### Remaining validation

Synthetic 16 kHz speech probes triggered the actual Linux Vosk library for a standalone wake and a one-breath instruction. The narrow CPU decoder also confused “connect that cable” with a wake candidate; the independent selected-ASR prefix check rejects that transcript. The core has 91 tests, the listener crate has 4, and coordinator tests cover rejecting ambient work while busy. This is not a microphone/accent false-activation benchmark. Real Windows microphone, foreground field, clipboard, follow-up timing, and installer setup still need the actual desktop. Do not label those acceptance gates passed from CI alone.

## October 17 continuation: Dad's hands-free session and live-text corrections

Spec: PLAN.md §5a (session), §9a (live text is the source of truth), milestone M3a.

- **Session core** — `striper-core::dictation::HandsFreeSession`: Off / Listening / Paused. "mute"/"unmute" count only as the whole utterance with ≥400 ms silence on both sides; while Paused everything except "unmute" is `Discard`, and utterances over 2 s are never transcribed. "correct that …" at the start of an utterance routes to Lane B (alone, the next utterance is the instruction). `apply_spoken_formatting` turns "new paragraph"/"new line" into breaks; `join_utterance` spaces and capitalizes across utterances.
- **Session app glue** — `src-tauri/src/striper/hands_free.rs`. A segmenter thread takes idle mic frames through `start_listener_capture` (same recorder as PTT), runs the selected VAD per frame and cuts utterances at 700 ms of silence (90 ms onset, 300 ms pre-roll, 25 s cap). A worker thread transcribes them in order with the selected recognizer, asks the core what they mean, and pastes with `clipboard::paste_exact` (no trailing space, never auto-submit) followed by `record_commit_exact`. The wake listener stands down while a session runs. Off stops capture.
- **Controls** — General page: large Start/End dictation button with Listening/Paused/Off text, plus the setting "Dictation shortcut starts and ends hands-free dictation" (`hands_free_shortcut`; off by default so Handy's push-to-talk is unchanged). `--toggle-dictation-session` for scripts. Sounds: Start on Listening, Stop on Paused, two Stops on Off. Overlay messages on every change.
- **Paragraph corrections** — new `insert_paragraph_breaks { after: [...] }` operation (command schema, validation, resolver, AI prompt). It only replaces the whitespace after each anchor with a blank line (`\r\n` kept where the field uses it).
- **Live text** — `striper-core::live`: `LiveField` trait, `context_window` (2,400 chars around the caret, mostly before it), `plan` (resolve against what the interpreter saw), `apply` (re-read; refuse if changed; select, read back, replace, verify, undo on mismatch, restore caret), `run` (re-plans once after a concurrent change), `undo`/`plan_undo`. `SimulatedLiveField` for tests.
- **Platform** — `striper-platform::live_text`: `read_focused_text` / `select_range`. Windows uses UI Automation (`live_text_windows.rs`; TextPattern + TextPattern2 caret, ValuePattern fallback for Edit/Document/ComboBox, password → refused, selection moves corrected by reading the range back). Linux returns `Unsupported` until AT-SPI exists.
- **Correction routing** — `correction::handle_instruction_guarded` reads the live field first. If readable and selectable it runs `handle_live` (live prompt via `build_live_user_message`, live undo history in `StriperState::live_history`; "scratch that" and undo-without-live-history fall back to the ledger). Protected fields are refused. Anything else uses the unchanged ledger path, so Linux behaves as before.

### Verified here (Linux container)

- striper-core: 123 tests pass, clippy `-D warnings` clean. striper-platform: tests pass; Windows code `cargo check`/`clippy --target x86_64-pc-windows-gnu` clean.
- src-tauri: `cargo check`, clippy (no new warnings in Striper code), `cargo test --lib striper::` (endpointer tests) pass. Bindings regenerated.
- Frontend: lint, build, prettier, translations pass. New strings are English in every locale.

### Needs Ben's machine / Windows

- Everything audible or on screen: endpointing timings with a real mic, Whisper hallucinations on noise, sounds, overlay, the button's look.
- UIA adapter has never run: test reading and selecting in Notepad, Word, Edge/Chrome text areas, Outlook; then the §9a acceptance list (typed text, yesterday's letter, typing while the AI thinks).
- Not yet built: Vosk grammar for the paused "unmute" listener (currently VAD + local ASR on short utterances), AT-SPI adapter for KDE, selection-only fallback in the app (core supports it; Ctrl+C-based selection reading is unsafe in apps that copy the whole line when nothing is selected), a distinct Off sound asset.
- After a live edit the ledger is invalidated (its caret-relative region no longer holds), so "scratch that" right after a live correction reports nothing recent.

## Where the code is

| Path                                  | What it holds                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| ------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crates/striper-core/`                | Tauri-free logic, fully unit-tested. `vocabulary.rs` (contextual replacement rules), `command.rs` (operation schema and validation), `parser.rs` (local undo/done/cancel controls only), `edit.rs` (diff → keystroke plan, `TextTarget` trait, verified execution, `SimulatedField`), `session.rs` (ledger of recent dictations, undo history, correction session state machine), `store.rs` (SQLite, migrations, audit log, export/import), `prompt.rs` (LLM prompt). |
| `crates/striper-platform/`            | Foreground window identity and "are modifiers held" for Windows (Win32) and X11; Wayland via `kdotool` if installed, otherwise unknown.                                                                                                                                                                                                                                                                                                                                |
| `src-tauri/src/striper/`              | App glue. `mod.rs` (state, init, Lane A hook, ledger recording), `correction.rs` (Lane B flow), `target.rs` (real `TextTarget`: keystrokes + clipboard read-back, clipboard save/restore), `keys.rs` (enigo / xdotool / wtype / dotool / ydotool key sender), `commands.rs` (Tauri commands for the UI).                                                                                                                                                               |
| `src-tauri/src/actions.rs`            | Handy's transcribe action. `TranscribeAction.correction` routes the "correct" binding to `striper::handle_instruction`; normal dictation calls `striper::record_commit` after pasting.                                                                                                                                                                                                                                                                                 |
| `src/components/settings/vocabulary/` | Vocabulary page: correction toggles, AI provider (shared with Post Process), teach a spelling, learned list, import/export, recent corrections.                                                                                                                                                                                                                                                                                                                        |
| `src/overlay/RecordingOverlay.tsx`    | `feedback` state shows each correction's result.                                                                                                                                                                                                                                                                                                                                                                                                                       |

Data lives in the app data dir (`com.benstein1.striper`): `settings_store.json` as in Handy, plus `striper.db` (SQLite) for vocabulary, audit log and edit transactions.

## Conversational AI correction (2026-10-17)

- Corrections are AI-only. The regex edit parser was removed on Ben's instruction; `parser::local_control` recognises only undo/done/cancel. Without an AI provider, edits are refused with a setup message. Do not reintroduce regex parsing of corrections.
- Vocabulary has a **Use GPT-6 Luna · Medium** preset (`gpt-6-luna`, explicit `reasoning_effort: medium`), a reasoning selector, and **Test AI correction**. The test sends only a synthetic sample and resolves returned operations in memory; no clipboard or desktop access.
- Provider/key/model remain shared with Post Processing. The preset clears legacy correction overrides so the visible provider controls match it. Normal dictation remains local. Custom OpenAI-compatible base URLs remain available; merge.dev integration awaits verification of its actual endpoint/protocol and credentials.
- Explicit reasoning effort is not silently dropped/retried. Provider default omits the effort parameter. Anthropic-format endpoints require Provider default. AI failures never fall back to regex edits; timeout is 45 seconds.
- Local verification: 93 core tests; 20 LLM client tests via a temporary isolated harness importing the real client (mock provider settings; rustls transport); frontend type-check, lint, build, translations and formatting. Full native app is verified by CI separately. No live OpenAI call was made here; Ben must enter his API key and run Test AI correction before the desktop demo. New locale keys use English pending translation.

## How a correction runs

1. The "correct" binding records and transcribes exactly like dictation (same coordinator, mic and model).
2. `correction::handle_instruction` routes controls locally and edits to configured AI (45 s timeout; three failures in a row pause it for 5 minutes).
3. On a blocking thread: wait for modifier keys to be released, check the focused window is still the one dictated into (`Ledger::check_target`), save the clipboard.
4. `session::run_operations` resolves each operation against the ledger region (the last few dictations into that window), plans the smallest edit, and executes it: move left over the unchanged suffix, Shift+Left over the expected text, copy it and compare, then paste or delete, then select and compare again. A mismatch after the edit triggers one Ctrl+Z and a check that the original is back.
5. Restore the clipboard, record the transaction, play a sound, show the overlay message, emit `correction-result`.

A word is saved permanently only when the user's own words ask for it ("remember that"); see `command::instruction_requests_memory`.

## Verified so far

- `cargo test` in `crates/striper-core` (91 tests) and `crates/striper-platform`; both also run on Windows CI (`.github/workflows/striper-core.yml`).
- `cargo test` in `src-tauri` (284 tests, Linux, at the voice-listener checkpoint), `cargo clippy` without new warnings, `bun run lint`, `bun run build`, `bun run format:check`, `bun run check:translations`.
- The striper-platform Windows code type-checks for `x86_64-pc-windows-gnu`.
- PLAN.md §0 (teach "Striper" without breaking "striped"/"stripper") and the Thursday → Friday → Saturday → undo flow pass against the simulated text field.

## Not verified (needs Dad's Windows machine or a real desktop)

- Anything with real keystrokes, clipboard, microphone or GPU: Notepad, Word, Outlook/web mail, the GTX 1080, timing.
- Apps that block copying make read-back impossible; the edit is then refused unless "Check before editing" is turned off.
- KDE Plasma Wayland focus and key injection; X11 is the better-supported Linux path.
- The vocabulary page and overlay feedback have not been looked at on screen.
- The first Windows CI build of the full app.

## Known open issues

- **Existing Striper installs:** automatic Handy import only runs before the first Striper settings store is created. It does not merge into an already configured Striper profile. Shared Hugging Face caches continue to be discovered by the model manager.
- **Updater.** `tauri.conf.json` still carries Handy's updater public key; update checks default to off. Generate a Striper key pair before enabling releases.
- **Translations.** New Striper strings are English in every locale.
- **Correction messages** in the overlay come from `StepOutcome::message()` in English, not i18n.
- **Wake tuning:** the opt-in CPU listener works in a synthetic probe; real microphone latency and false activations remain unmeasured. Unknown desktop focus refuses ambient activation.
- **Paste trimming.** Handy options such as trailing-space handling run on dictation, not on correction edits; check "two spaces" on Windows.

## Next steps, in order

1. Prioritize Windows 11 acceptance for Dad. Use the Windows guest checklist in `docs/LINUX_TESTING.md` when a Windows VM is available (WinBoat is Ben's likely route). Use the Linux desktop for shared-code iteration and supporting smoke tests.
2. Try Dad's hands-free session (PLAN.md §5a acceptance list) with a real microphone, then the UIA live-text checks in §9a on Windows Notepad and Word.
3. Check the compatibility panel and fix actual focus/input/clipboard failures before testing the voice listener.
4. Run PLAN.md §0 (learn “Striper”, restart, preserve “striped shirt”) and the Thursday → Friday → Saturday → undo sequence in Windows Notepad and a real email/word-processing workflow. Track guest results separately from Dad's PC.
5. Test wake confirmation, same-breath commands, standalone wake acknowledgment, follow-ups, cancellation, and focus changes with the actual microphone.
6. Batch fixes and request the installer needed for the next actual test: Windows for the Windows guest, Linux for local installation. Retain cross-platform tests. Do not launch repeated installer builds or defer Windows/Word work merely because development happens on Linux.
7. Generate Striper updater keys before enabling self-updates. Consider a Windows UI Automation/Word adapter only after actual app results justify it.

## Working on it

- Regenerate `src/bindings.ts` after changing a Tauri command: `cargo build` in `src-tauri`, then run the debug binary once. On a headless box: `LD_LIBRARY_PATH=$(ls -d target/debug/build/transcribe-cpp-sys-*/out/build/src) ./target/debug/striper`. It writes the bindings before it fails for lack of a display.
- Keep new logic in `striper-core` where it can be tested without Tauri; `src-tauri/src/striper/` should stay thin.
- Commit straight to `main` (owner's choice), with conventional commit prefixes.
