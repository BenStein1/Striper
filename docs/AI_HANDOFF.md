# Striper: AI handoff

Read [PLAN.md](../PLAN.md) first; it is the spec, and its "Implementation status" table is the current scoreboard. This file is the short version for the next engineer or agent.

## Current development policy

- Linux desktop or a VM is the active manual-test target. Preserve Windows support; real Windows validation is deferred.
- Installer packaging and full Nix builds are manual. Do not launch repeated builds while iterating. Main Branch Build defaults to Linux and offers Windows/both when explicitly requested.
- Use local core/platform/listener tests and frontend checks for iteration, then batch a testable set of changes. See [LINUX_TESTING.md](LINUX_TESTING.md).
- Vocabulary now includes a read-only compatibility panel and a copyable diagnostic report. It reports helper detection separately from working device/compositor permissions.
- KDE idle focus checks use one minimal query at most four times per second. Every kdotool query is bounded to 250 ms; edit-time focus checks stay uncached and fail when a previously known window becomes unobservable.

## What Striper is

A fork of [Handy](https://github.com/cjpais/Handy) (Tauri 2: Rust backend, React frontend) for a Dragon user on Windows 11 with limited hand mobility. Two lanes share one microphone and one Whisper model:

- **Lane A, dictation.** Hold Ctrl+Space, speak, release. Handy's pipeline, plus learned spellings applied before the text is pasted. Never touches the LLM.
- **Lane B, correction.** Enable the local CPU listener in Vocabulary settings, then say “correct that” and what to change about recent dictation. Follow-ups remain voice-only for 30 seconds. Ctrl+Alt+Space (or `striper --toggle-correction`) is the fallback. A local parser handles common phrasings; an OpenAI-compatible LLM is asked only when the parser gives up. Either way the result is a small list of validated `EditOperation`s that Rust executes and verifies. The LLM never gets keystrokes, shell or desktop access.

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

## Where the code is

| Path                                  | What it holds                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| ------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `crates/striper-core/`                | Tauri-free logic, fully unit-tested. `vocabulary.rs` (contextual replacement rules), `command.rs` (operation schema and validation), `parser.rs` (deterministic instruction parser), `edit.rs` (diff → keystroke plan, `TextTarget` trait, verified execution, `SimulatedField`), `session.rs` (ledger of recent dictations, undo history, correction session state machine), `store.rs` (SQLite, migrations, audit log, export/import), `prompt.rs` (LLM prompt). |
| `crates/striper-platform/`            | Foreground window identity and "are modifiers held" for Windows (Win32) and X11; Wayland via `kdotool` if installed, otherwise unknown.                                                                                                                                                                                                                                                                                                                            |
| `src-tauri/src/striper/`              | App glue. `mod.rs` (state, init, Lane A hook, ledger recording), `correction.rs` (Lane B flow), `target.rs` (real `TextTarget`: keystrokes + clipboard read-back, clipboard save/restore), `keys.rs` (enigo / xdotool / wtype / dotool / ydotool key sender), `commands.rs` (Tauri commands for the UI).                                                                                                                                                           |
| `src-tauri/src/actions.rs`            | Handy's transcribe action. `TranscribeAction.correction` routes the "correct" binding to `striper::handle_instruction`; normal dictation calls `striper::record_commit` after pasting.                                                                                                                                                                                                                                                                             |
| `src/components/settings/vocabulary/` | Vocabulary page: correction toggles, AI provider (shared with Post Process), teach a spelling, learned list, import/export, recent corrections.                                                                                                                                                                                                                                                                                                                    |
| `src/overlay/RecordingOverlay.tsx`    | `feedback` state shows each correction's result.                                                                                                                                                                                                                                                                                                                                                                                                                   |

Data lives in the app data dir (`com.benstein1.striper`): `settings_store.json` as in Handy, plus `striper.db` (SQLite) for vocabulary, audit log and edit transactions.

## How a correction runs

1. The "correct" binding records and transcribes exactly like dictation (same coordinator, mic and model).
2. `correction::handle_instruction` parses locally, else calls the LLM (15 s timeout; three failures in a row pause it for 5 minutes).
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

1. Follow `docs/LINUX_TESTING.md` on the Linux desktop or a VM, starting with a disposable plain-text document and the shortcut path.
2. Check the compatibility panel and fix actual focus/input/clipboard failures before testing the voice listener.
3. Run PLAN.md §0 (learn “Striper”, restart, preserve “striped shirt”) and the Thursday → Friday → Saturday → undo sequence on that desktop.
4. Test wake confirmation, same-breath commands, standalone wake acknowledgment, follow-ups, cancellation, and focus changes with the actual microphone.
5. Batch fixes and request a Linux installer only when useful. Windows installers and Windows/Word acceptance remain deferred; retain cross-platform tests.
6. Generate Striper updater keys before enabling self-updates. Consider a Windows UI Automation/Word adapter only after actual app results justify it.

## Working on it

- Regenerate `src/bindings.ts` after changing a Tauri command: `cargo build` in `src-tauri`, then run the debug binary once. On a headless box: `LD_LIBRARY_PATH=$(ls -d target/debug/build/transcribe-cpp-sys-*/out/build/src) ./target/debug/striper`. It writes the bindings before it fails for lack of a display.
- Keep new logic in `striper-core` where it can be tested without Tauri; `src-tauri/src/striper/` should stay thin.
- Commit straight to `main` (owner's choice), with conventional commit prefixes.
