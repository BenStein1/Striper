# Striper: AI handoff

Read [PLAN.md](../PLAN.md) first; it is the spec, and its "Implementation status" table is the current scoreboard. This file is the short version for the next engineer or agent.

## What Striper is

A fork of [Handy](https://github.com/cjpais/Handy) (Tauri 2: Rust backend, React frontend) for a Dragon user on Windows 11 with limited hand mobility. Two lanes share one microphone and one Whisper model:

- **Lane A, dictation.** Hold Ctrl+Space, speak, release. Handy's pipeline, plus learned spellings applied before the text is pasted. Never touches the LLM.
- **Lane B, correction.** Hold Ctrl+Alt+Space (or run `striper --toggle-correction`) and say what to change about recent dictation. A local parser handles common phrasings; an OpenAI-compatible LLM is asked only when the parser gives up. Either way the result is a small list of validated `EditOperation`s that Rust executes and verifies. The LLM never gets keystrokes, shell or desktop access.

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

- `cargo test` in `crates/striper-core` (86 tests) and `crates/striper-platform`; both also run on Windows CI (`.github/workflows/striper-core.yml`).
- `cargo test` in `src-tauri` (281 tests, Linux), `cargo clippy` without new warnings, `bun run lint`, `bun run build`, `bun run format:check`, `bun run check:translations`.
- The striper-platform Windows code type-checks for `x86_64-pc-windows-gnu`.
- PLAN.md §0 (teach "Striper" without breaking "striped"/"stripper") and the Thursday → Friday → Saturday → undo flow pass against the simulated text field.

## Not verified (needs Dad's Windows machine or a real desktop)

- Anything with real keystrokes, clipboard, microphone or GPU: Notepad, Word, Outlook/web mail, the GTX 1080, timing.
- Apps that block copying make read-back impossible; the edit is then refused unless "Check before editing" is turned off.
- KDE Plasma Wayland focus and key injection; X11 is the better-supported Linux path.
- The vocabulary page and overlay feedback have not been looked at on screen.
- The first Windows CI build of the full app.

## Known open issues

- **Data folder moved.** The identifier changed from Handy's `com.pais.handy`, so an existing Handy install's models and settings are not picked up. Copy `%APPDATA%\com.pais.handy` to `%APPDATA%\com.benstein1.striper`, or add a one-time migration.
- **Updater.** `tauri.conf.json` still carries Handy's updater public key; update checks default to off. Generate a Striper key pair before enabling releases.
- **Translations.** New Striper strings are English in every locale.
- **Correction messages** in the overlay come from `StepOutcome::message()` in English, not i18n.
- **No wake phrase yet.** PLAN.md M3; the hotkey is the fallback.
- **Paste trimming.** Handy options such as trailing-space handling run on dictation, not on correction edits; check "two spaces" on Windows.

## Next steps, in order

1. Install the CI artifact on Windows 11. Run Lane A as before, then the M1 demo in Notepad (dictate, "change Thursday to Friday", "actually Saturday", "undo that").
2. Run the §0 test for real: dictate "I opened striped", "correct that; I meant Striper, S-T-R-I-P-E-R, remember that", restart, dictate again, and check that "striped shirt" is unchanged.
3. Fix what breaks in `target.rs` and `keys.rs` (timing, per-app quirks) and add an app capability list.
4. Data migration from `com.pais.handy`, then updater keys.
5. Wake phrase: a CPU keyword spotter fed from the existing always-on mic stream, gated off during push-to-talk.
6. UI Automation adapter for Word if the keystroke path falls short.

## Working on it

- Regenerate `src/bindings.ts` after changing a Tauri command: `cargo build` in `src-tauri`, then run the debug binary once. On a headless box: `LD_LIBRARY_PATH=$(ls -d target/debug/build/transcribe-cpp-sys-*/out/build/src) ./target/debug/striper`. It writes the bindings before it fails for lack of a display.
- Keep new logic in `striper-core` where it can be tested without Tauri; `src-tauri/src/striper/` should stay thin.
- Commit straight to `main` (owner's choice), with conventional commit prefixes.
