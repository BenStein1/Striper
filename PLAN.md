# Striper — Development Plan

**Status:** In implementation. See [Implementation status](#implementation-status) and [docs/AI_HANDOFF.md](docs/AI_HANDOFF.md)  
**Date:** October 17, 2026  
**Product priority:** Windows 11 for Dad is the primary user experience and acceptance target. Linux remains supported.
**Development/test environment:** Ben develops on KDE Plasma Linux. A Windows 11 VM, potentially through WinBoat, is the planned Windows test route; Dad's own PC remains the final hardware/application acceptance target.
**Audience:** A Dragon NaturallySpeaking user on Windows and a Linux developer. See [desktop and VM testing](docs/LINUX_TESTING.md) for Linux iteration and Windows acceptance checks.
**Application name:** Striper (S-T-R-I-P-E-R), named for the striped bass; not “striped” or “stripper.”

## Implementation status

_Last updated October 17, 2026. "Tested" means automated tests on Linux (and Windows CI for the core crates). "Needs Windows" means the code exists but has not run on Dad's Windows 11 machine, his microphone or the GTX 1080. Checkboxes in the sections below stay unticked until their acceptance gate has passed on real hardware._

| Area | State | Evidence |
|---|---|---|
| Fork, rebrand to Striper, Handy attribution kept | Done | `9bf8a10`; About page and README credit Handy; original icon kept at `assets/branding/striper-original-icon.png` |
| Installer builds (Windows NSIS/MSI; Linux deb/AppImage) | Manual, platform selectable | `.github/workflows/main-build.yml`; no installer build on routine commits |
| Shared core crate (`crates/striper-core`): vocabulary, command schema, ledger, verified edit planner, SQLite store | Done, tested | 91 unit tests; Linux and Windows CI |
| Platform crate (`crates/striper-platform`): foreground window + modifier state, Windows and X11 (KDE Wayland via kdotool if installed) | Done, Windows path type-checked only | Needs Windows |
| Lane A: learned vocabulary applied before paste, context-aware, per-app scope, ASR hints | Done, tested in core | Needs Windows |
| Lane A keeps working with no LLM | Done | Lane A never calls the LLM |
| Lane B: correction shortcut (Ctrl+Alt+Space) and `--toggle-correction`, shared mic/ASR path with Lane A | Done | Needs Windows |
| Lane B operations: replace, follow-up ("actually Saturday"), punctuation, spaces, insert, capitalize, delete, scratch that, undo, remember, forget, cancel, done | Done, tested in core | Needs Windows (Notepad, Word, email) |
| Corrections interpreted by the OpenAI-compatible LLM only (strict schema, timeout and back-off); undo/done/cancel recognised locally | Done | Needs a live endpoint. Regex edit parsing removed on Ben's instruction (Oct 17) |
| Verified, transactional, reversible edits (selection read-back, post-verify, undo on mismatch, clipboard restore, stale-focus check) | Done, tested against a simulated field | Needs Windows; read-back depends on each app allowing copy |
| Persistent memory only when the user says "remember", stored in SQLite with audit log | Done, tested | §0 acceptance test passes in core tests; needs a real restart on Windows |
| Vocabulary manager UI, import/export, recent corrections, overlay feedback | Done | Lint and build pass; not yet viewed on screen |
| Wake phrase "correct that" (CPU detector) | Implemented, desktop validation pending | Opt-in native Vosk listener behind a VAD speech gate; holds the mic only for 60 s after push-to-talk dictation or a correction, never after a hands-free session; shared capture; selected ASR for instructions; synthetic Linux acoustic probe passed |
| Hands-free dictation session (Ctrl+Space toggle, Listening / Paused / Off, "mute"/"unmute", "new paragraph") | Implemented, core and endpointing tested | §5a. `striper-core::dictation` (16 tests), `src-tauri/src/striper/hands_free.rs` (VAD endpointing tests), big Start/End button on General, `--toggle-dictation-session`. Needs a real microphone, then Windows. Paused listener is VAD + duration gate + local ASR; the Vosk "unmute" grammar is not wired yet |
| Paragraph corrections ("new paragraph after …", "split into paragraphs") | Done, tested in core | `insert_paragraph_breaks` operation changes whitespace only; produced by the AI |
| Live-text source of truth for "correct that" (read field, plan, verify unchanged, apply) | Implemented, tested against a simulated field | §9a. `striper-core::live` (14 tests): re-plans once if the field changed, verifies selection and result, restores the caret, live undo/follow-ups. Used whenever the platform can read the field; otherwise the ledger path |
| Windows UI Automation / Word adapter | UIA reader implemented, type-checked only | `crates/striper-platform/src/live_text_windows.rs`: TextPattern text/selection/caret, ValuePattern fallback, password refusal, verified range selection. Needs Windows (Notepad, Word, Edge). Word-specific adapter later |
| Linux AT-SPI live-text adapter (KDE) | Not started | §9a fallback order applies until it exists |
| Data migration from Handy's `com.pais.handy` app-data folder | Implemented, unit-tested | Fresh installations import settings and completed models without changing Handy or overwriting Striper |
| Updater signing keys for Striper | Not started | Update checks default off |

## 0. First acceptance test — Striper must remember its own name

**Problem:** Dictating “Striper” may yield “striped” or “stripper.” This is a small but useful end-to-end test of the promised correction and persistent vocabulary features.

1. During ordinary dictation, deliberately capture a misrecognized mention of the application's name, such as “I opened striped” or “launch stripper,” where the user really meant **Striper**.
2. Say “correct that; I meant Striper, S-T-R-I-P-E-R; remember that” (or an equivalent natural correction). Striper should identify the intended prior text, preview or safely apply the edit, and save the spelling as an explicit user-taught proper name.
3. Across later dictations and application restarts, recognize “Striper” in relevant contexts without making the user spell it again. Prefer a recognizer custom-word hint when available, with a validated post-transcription normalization fallback where necessary.
4. **Do not globally replace** the valid English words “striped” or “stripper” in unrelated sentences. Context matters; when intent is uncertain, ask or leave the transcript unchanged.
5. Confirm that undo/revert works, the saved preference can be reviewed or deleted, and the behavior passes in Windows 11 (initially a guest if needed), then on Dad's PC before release; Linux checks support development and cross-platform compatibility.

This requirement is part of the first milestone, not a cosmetic cleanup. It proves the ASR -> correction -> persistent memory -> safe insertion loop while preserving normal dictation.

## 1. Mission

Fork and extend the existing **Handy** dictation experience rather than replace it. Prioritize a simple, reliable Windows 11 experience for Dad. Develop the shared code on Linux and build/test both Windows and Linux targets; use a Windows 11 VM for Windows acceptance when his PC is unavailable. Normal dictation should remain quick and familiar: hold **Ctrl+Space**, speak, release, and the words appear in the active application. Add a second, conversational lane activated by **“correct that”** (with a keyboard fallback). The user can then say what to change in ordinary English, and the system safely updates the relevant text.

A correction such as “My first name is Stephen, S-T-E-P-H-E-N; remember that” should persist across restarts. We should not have to retrain Whisper or pay for a Dragon upgrade.

**Design principle:** The speech recognizer transcribes; a command interpreter understands intent; a deterministic editor makes changes; a local dictionary remembers preferences. **The LLM must never receive unrestricted control of Windows.**

## 2. Goals and non-goals

### Must have

- [ ] Keep the current Handy **Ctrl+Space** dictation flow and its working GPU-accelerated Whisper setup.
- [ ] Support most ordinary Windows text fields, initially with explicit supported-app coverage.
- [ ] Activate correction mode by voice (“correct that”) without requiring rigid Dragon command syntax.
- [ ] Understand edits such as “change Thursday to Friday,” “replace that question mark with a period,” “put two spaces after the sentence,” “scratch that,” and “undo that.”
- [ ] Support follow-up commands in the same editing session: “Actually, make it Saturday.”
- [ ] Offer a **hands-free dictation session** (§5a): one Ctrl+Space press (or a large Start dictation button) starts it, speech is committed at natural pauses, “mute”/“unmute” pause and resume by voice, and a second press ends it. Three unmistakable states: **Listening, Paused, Off**.
- [ ] Support Dragon's spoken **“new paragraph”** during dictation, and paragraph restructuring afterwards through “correct that” without changing the wording.
- [ ] Treat the **focused application's live text** as the source of truth for corrections (§9a), so the user can correct text he typed himself or a letter opened from yesterday, not only text Striper remembers dictating.
- [ ] Remember learned names, spellings, and explicit user preferences across sessions, beginning with the app's own name, **Striper**.
- [ ] Make edits reversible and refuse unsafe/ambiguous edits rather than guessing.
- [ ] Offer an **OpenAI-compatible** LLM endpoint (local or hosted); keep normal dictation working if it is unavailable.
- [ ] Maintain one cross-platform core and platform-specific text-editing adapters for Windows and Linux.
- [ ] Support development on Linux while keeping Windows 11 the primary product target; run Windows tests in CI and interactive acceptance in a Windows guest and Dad's own environment. Batch manual installer builds rather than packaging each edit.
- [ ] Ship a simple Windows installer and Linux package/tray app with accessible visual/audio status and minimal setup.

### Nice to have after MVP

- [ ] Optional local LLM execution, fully offline mode, voice-based vocabulary review.
- [ ] Select phrase / select sentence / replace selected text by speech.
- [ ] Word-specific formatting, number formatting, punctuation styles, per-application profiles.
- [ ] Export/import dictionary and profiles; advanced Linux desktop adapters (X11 and Wayland) and additional application-specific integrations.

### Not in MVP

- Full Dragon feature parity, voice macros for arbitrary desktop automation, browser navigation, or filesystem control.
- Blind editing of entire documents or unbounded clipboard history. (Whole-document corrections are allowed only where an accessibility API actually exposes the whole document, §9a.)
- Guaranteed editing in every Windows control, secure desktop, password field, or elevated app.
- Continuous recording or cloud audio upload without clear opt-in.

## 3. Current Handy foundation (verify against pinned release before coding)

The upstream project is **[cjpais/Handy](https://github.com/cjpais/Handy)**, a Rust/Tauri application with local Whisper/Parakeet dictation and Windows support. Its current README documents remote commands including:

```text
handy --toggle-transcription
handy --toggle-post-process
handy --cancel
```

The repository also exposes custom words, experimental LLM post-processing, and a configurable custom provider in its settings/source. **These features do not imply a supported plugin API for intercepting transcribed text, a stable edit API, or a transcript-insertion event.** Those details need code inspection and testing.

Initial investigation checklist:

- [ ] Pin exact upstream commit and installed Handy version used on the GTX 1080 PC.
- [ ] Trace the pipeline: microphone -> VAD -> ASR -> custom-word normalization -> optional post-processing -> paste/typing.
- [ ] Identify any pre-insertion hook, external-script hook, event, or safe patch point.
- [ ] Determine how Handy captures the destination HWND, cursor/focus, insertion text, and clipboard restoration.
- [ ] Confirm whether post-processing can return structured data or only replacement transcript text.
- [ ] Benchmark GPU VRAM occupancy, dictation latency, and speech engine lifetime on the GTX 1080.
- [ ] Review Handy license and obligations before distributing a fork.

**Decision gate:** Fork Handy and prefer the smallest maintainable in-tree change that adds an authoritative pre-insertion/commit hook if none exists. A separate sidecar can own wake-word detection, editing, settings, and persistence, but it should receive an authoritative `TranscriptCommitted` event from Handy. Avoid unreliable clipboard polling or inferring edits by watching keystrokes as the main architecture.

## 4. Two-lane architecture

```text
                          MICROPHONE (shared capture where possible)
                                       |
                 +---------------------+---------------------+
                 |                                           |
       LANE A: NORMAL DICTATION                    LANE B: COMMAND LISTENER
       Ctrl+Space -> Handy ASR                    low-power wake-phrase detector
                 |                                   "correct that"
          raw transcription                                  |
                 |                                capture instruction audio
        deterministic dictionary                             |
                 |                               existing Handy ASR if safely
      optional text-only cleanup LLM             callable, otherwise separate ASR
                 |                                           |
      commit to focused app + anchor               command interpreter LLM
                 |                                           |
            transaction log                      validated edit operation(s)
                 +---------------------+---------------------+
                                       |
                    Platform-neutral Edit Coordinator
                        app adapter + range validation
                                       |
                         change / verify / undo
                                       |
                      SQLite vocabulary + edit history
```

**No automatic cloud calls on Lane A by default.** Lane A's preference substitutions should be deterministic and fast. An optional text-cleanup LLM is a user setting, not a dependency. Lane B can use a hosted fast model through an OpenAI-compatible API, or a local model if hardware and latency permit. Inference-provider identity, credit amount, and free-tier rules must be verified separately; do not bake a particular provider into the app.

### Modules

1. **Handy bridge** (Rust): authoritative pre-paste/post-commit event with raw/final text, destination HWND/control identity where available, cursor/selection details where available, timestamps, and transaction ID.
2. **Wake listener** (CPU): small continuously running detector; no full-time Whisper inference. Explore [openWakeWord](https://github.com/dscripka/openWakeWord) with a **custom-trained phrase**. Add VAD, threshold tuning, false-activation testing, and hotkey fallback.
3. **Command session controller**: explicit idle/listening/thinking/applying/confirmation states, context timeout, cancellation, and audio feedback.
4. **Command interpreter**: bounded context to a fast LLM; returns structured edit intents, not keystrokes or prose.
5. **Platform-neutral Edit Coordinator**: read/select/replace within proven target bounds, dispatch to Windows/Linux adapters, verify result, register undo.
6. **Learning dictionary**: persistent spellings/aliases/pronunciations, context and confidence rules, reversible preferences.
7. **Settings/tray UX**: provider configuration, offline/online indicator, wake-word toggle, microphone state, vocabulary manager, diagnostics; implement cross-platform UI while isolating platform-dependent tray/hotkeys.

## 5. Interaction contract

### Everyday dictation

1. User focuses Word, Notepad, Outlook, a browser field, etc.
2. Holds **Ctrl+Space** and dictates.
3. Handy produces text.
4. **Before insertion**, local vocabulary rules correct high-confidence known terms.
5. Handy inserts the final text. Bridge records the committed text, target identity, and best available anchor.

### Correction session

1. User says **“correct that”** (or uses configurable hotkey). Show a brief visual/audio acknowledgment.
2. Wake listener pauses while command capture is active to prevent recursive activations.
3. Record one instruction using VAD/silence timeout. Use Handy's ASR if a supported bridge exists; otherwise a separate ASR path with coordinated microphone access.
4. Pass the instruction plus *only necessary, recent* dictation context to the LLM.
5. LLM returns one or more **schema-validated edit intents**.
6. Coordinator validates the target, expected text, current app focus, edit range, and recency; applies the change if safe.
7. Confirm success unobtrusively. Keep the session open briefly for follow-up instructions; end on “done,” “cancel,” timeout, or app-focus change.

**Speech-mode rule:** Outside correction mode, ordinary words—including the literal phrase “correct that” in dictated prose—must not silently rewrite existing text. Tune wake detection/activation accordingly; offer an explicit hotkey to resolve ambiguity.

### Examples

| Spoken instruction | Intended action |
|---|---|
| “Correct that. I said Stephen, S-T-E-P-H-E-N. Remember it.” | Correct recent name and store conditional spelling preference. |
| “Correct that. Change Thursday to Friday.” | Replace the unique occurrence in the anchored recent text. |
| “Actually, make it Saturday.” | Use the previous transaction/session target and revise Friday -> Saturday. |
| “Change the question mark to a period and add two spaces.” | Modify punctuation and trailing spaces if the target app preserves them. |
| “Scratch that.” | Remove the last reliably anchored dictation transaction. |
| “Undo that.” | Reverse the last **assistant edit**, verifying the current target first. |
| “Don't remember that spelling.” | Remove/disable the stored preference; preserve an audit entry. |

## 5a. Hands-free dictation session (Dad's Dragon workflow)

**Source:** Ben's conversation with his dad about how he uses Dragon (October 17, 2026). Dad switches Dragon on once, talks to write a letter, says “mute” to stop it listening and “unmute” to carry on, and says “new paragraph” where he wants one. He should not have to touch the keyboard between thoughts: at most one keystroke to start and one to end.

This mode is **optional and additive**. Hold-to-talk Ctrl+Space (Lane A) keeps working exactly as before for anyone who prefers it; a setting chooses which behavior Ctrl+Space has.

### Controls and states

| He does or says | Striper does |
|---|---|
| Presses **Ctrl+Space** once, or clicks the large **Start dictation** button | Starts the session: **Listening**. Subtle start sound. |
| Speaks normally | Dictates. Each utterance is committed (vocabulary-corrected, then inserted) when he pauses naturally. |
| “New paragraph” while dictating | Inserts a paragraph break at that point instead of the words. |
| “Correct that, start a new paragraph after ‘see you tomorrow’” | Finds that phrase in the live text and inserts a paragraph break after it (§9a). |
| “Correct that, split this into paragraphs” | Re-paragraphs the recent passage **without changing a single word** (§9a validation). |
| “Mute” (on its own) | Pauses: **Paused** indicator and a subtle sound. |
| “Unmute” (on its own) | Resumes: **Listening** indicator and a subtle sound. |
| Presses **Ctrl+Space** again, or clicks **End dictation** | Ends the session, including the paused “unmute” listener: **Off**. |

- There are exactly three states, **Listening, Paused, Off**, each with its own visual indicator (main window button, overlay and tray) and its own subtle sound on entry. Sounds follow the existing audio-feedback setting.
- The main window shows one large **Start dictation / End dictation** button reflecting the current state; Ctrl+Space does the same thing.
- The cancel shortcut and the tray remain an emergency stop; cancelling discards the utterance in flight but leaves the session state unchanged unless the user ends it.

### Mute and unmute

- “Mute” and “unmute” count **only as standalone utterances with a pause on either side**. The whole recognized utterance must be the command word (punctuation and case ignored), and the silence before and after it must each meet a minimum (initially 400 ms; tune on the real microphone). “I put my phone on mute” is ordinary dictation.
- **While paused, only a small local listener runs, and it listens only for “unmute.”** Every other utterance is discarded: never inserted, never stored, never sent to the LLM. Long utterances are dropped without being transcribed at all; short ones may be checked locally. Prefer the CPU keyword listener (Vosk grammar limited to “unmute”) when it is installed, confirmed by the local recognizer; otherwise the Silero VAD plus a short-duration gate and local recognizer.
- **Ending the session stops the paused listener too.** Off means no microphone use by the session at all.

### Paragraphs

- During dictation, the spoken phrase “new paragraph” (and “new line” for a single break) becomes a paragraph break, with the following word capitalized and stray punctuation/spaces around the command removed. Supporting this means Dad does not have to unlearn a useful Dragon habit.
- He can also speak his mind first and restructure afterwards with “correct that”: inserting a paragraph break after a named phrase, or splitting a passage into paragraphs. Paragraph-only operations are validated to change whitespace only; any change to the words is rejected.

### Correction inside a session

- An utterance that begins with “correct that” while Listening is a correction instruction (the same Lane B pipeline), not dictation. The rest of the utterance, or the next utterance if he pauses after the wake phrase, is the instruction. Follow-ups (“actually, Friday”, “undo that”) keep working as in §5.
- Correction reads the focused field's live text (§9a); session history is used only to interpret “undo that” and follow-ups.

### Acceptance

- [ ] With the keyboard used only to start and end: dictate a short letter with pauses, say “new paragraph” twice, say “mute”, talk to someone in the room (nothing inserted, nothing sent), say “unmute”, continue, end the session.
- [ ] “I put my phone on mute.” is inserted as text and does not pause.
- [ ] “Correct that, start a new paragraph after ‘see you tomorrow’” and “Correct that, split this into paragraphs” change only paragraph breaks.
- [ ] Each state is distinguishable by sight and by sound; Off releases the microphone.
- [ ] Passes on Windows 11 with Dad's microphone; Linux checks support development.

## 6. Persistent learning: treat it as a preference, not a magical ASR retrain

Store preferences in **SQLite**, independent of Whisper's model files and Handy settings. Keep a migration/version number and export/import support.

Proposed logical fields:

```text
vocabulary_entry
  id, preferred_text, aliases[], spoken_hint?, match_mode,
  context_rule?, scope(global|app|person|document),
  user_confirmed, enabled, created_at, updated_at

edit_transaction
  id, timestamp, app_id, target_fingerprint,
  source_transcript_id?, before_text, after_text,
  range_or_anchor?, intent, result, undo_state
```

**Rules:**

- Favor **explicitly user-confirmed** spellings for strong corrections; infer less from a single edit.
- Preserve case, punctuation, and word boundaries. Normalize cautiously: “Connor” and “Conner” are both real names.
- Only auto-replace ambiguous homophones when context or explicit scoped rules justify it. A global Connor -> Conner substitution is **not** safe by default.
- Offer exact spelling capture from dictated letter sequences, with a verification prompt if uncertain.
- Apply deterministic substitutions before insertion; the LLM may **propose** a new rule but never silently persist one.
- Maintain versioned undo/delete for dictionary rules; don't require cloud connectivity.
- Reuse learned vocabulary as optional Whisper prompting/context *and* deterministic post-ASR correction; the latter is the authoritative preference layer.

## 7. LLM interface and safety boundary

Expose a provider abstraction: `base_url`, `model`, `api_key` (stored with Windows credential protection where feasible), timeout, cost limits, and offline/online selection. Accept OpenAI-compatible chat-completion APIs through a small adapter; handle providers without reliable structured output via schema parsing + rejection on validation failure.

Illustrative command result **(contract proposal, not existing Handy API)**:

```json
{
  "intent": "replace_text",
  "target": "last_committed_transcript",
  "expected_text": "Thursday",
  "replacement_text": "Friday",
  "scope": "current_sentence",
  "confidence": 0.96,
  "persist_preference": false
}
```

Allowed operations: `replace_text`, `replace_punctuation`, `insert_text`, `insert_paragraph_breaks` (whitespace-only restructuring, §5a), `delete_recent_dictation`, `undo_last_edit`, `remember_spelling`, `forget_spelling`, `clarify`, `cancel`, `finish`.

**Hard constraints:**

- Reject all unknown operations, extra instructions, invalid JSON, targets outside the permitted area, and edits not matching current text.
- Never execute shell commands, open URLs, alter unrelated fields, or permit the model to send raw arbitrary key combinations.
- Ask the user to clarify if several matches or a stale cursor make the target ambiguous.
- Restrict content sent to external models to the requested edit plus a small recent-context window; never send entire windows or documents by default.
- No sensitive text editing or cloud transmission in password fields, secure desktop, or disallowed applications.
- LLM failure should not break dictation. Disable auto-application after repeated errors.

## 8. Cross-platform architecture and Linux development workflow

**Cross-platform is a core requirement, not a possible future enhancement.** Dad's Windows 11 machine is the first shipping target, but architecture, tests, and build tooling must also support Linux from the outset. We should fork Handy upstream, keep modifications small, and avoid introducing Windows-specific assumptions into the shared transcription, command interpretation, learning, or transaction logic.

Suggested Rust boundaries:

- `editor-core`: platform-independent structured intents, edit session state, anchoring rules, validation, transaction history, dictionary, and tests.
- `platform-windows`: HWND/UI Automation, Win32 focus/selection and clipboard integrations, per-app adapters.
- `platform-linux`: X11/Wayland detection, compositor/portal accessibility constraints, clipboard and input abstractions, per-app adapters.
- `handy-bridge`: a shared transcription/pre-insertion/commit event API within the Handy fork.
- `desktop-ui`: shared Tauri frontend plus narrowly scoped native integrations and a portable configuration store.

**Linux development:** Use the normal Rust/Tauri toolchain for Linux iteration and run shared tests locally. Have GitHub Actions (or an equivalent CI runner) build and test the Windows target natively on Windows workers, create Windows install artifacts, and run Windows UI automation integration tests where an interactive desktop is available. Cross-compiling a full Tauri application from Linux to Windows is not guaranteed to be practical; do not design delivery around that assumption. Run smoke tests on Dad's actual GTX 1080 Windows box before release.

**Linux runtime caveat:** X11 can often support global hotkeys and programmatic text insertion through desktop-specific interfaces, but Wayland deliberately limits global key observation, focus inspection, and synthetic input. Investigate desktop portals, accessibility APIs, compositor-specific extensions, and app integrations; never promise universal Wayland editing or bypass compositor security. Present a clearly scoped Linux capability matrix (KDE Plasma Wayland, Plasma X11, GNOME Wayland) and safe fallback modes. Privacy and permission prompts must remain explicit.

**Cross-platform acceptance gate:** The project must compile and run its shared Handy bridge, dictionary, command parser, and session logic on both Linux and Windows by M1. Demonstrate basic Linux dictation/correction in a supported field by M3, with wider Linux app coverage after Windows-first reliability is established.

## 9. Windows-wide editing strategy (critical technical risk)

**Do not claim universal editing.** Microsoft's UI Automation **TextPattern is mainly read-only**; read access does not imply arbitrary write access. Windows applications expose very different text control and selection APIs. Use a tiered approach:

1. **Tier 1 — committed Handy transaction:** Most reliable. If the destination, text, selection, and focus are unchanged, replace/undo *only the most recently inserted text*. This is the MVP target.
2. **Tier 2 — UI Automation selection and readback:** Discover text/range/caret with TextPattern/TextPattern2 or ValuePattern where available; edit through supported interfaces or carefully simulated selection and paste. Verify resulting text.
3. **Tier 3 — app-specific adapters:** Word via supported Office automation for precise document ranges where practical; native edit controls, browser fields, and Electron apps receive individual adapters/tests.
4. **Tier 4 — fail safely:** If the target cannot be identified or verified, show proposed corrected text for manual copy/paste; never blindly send Backspace or Ctrl+Z a guessed number of times.

Required safeguards:

- Capture stable window/control identity and the last Handy commit's text/selection whenever possible.
- Invalidate the anchor when another window gains focus, when the document changes unexpectedly, or after an expiry period.
- Before editing, compare the expected source text to the current range (optimistic concurrency check).
- Keep clipboard preservation/restoration bounded and test paste races and delayed consumers.
- Prefer native document undo integration when verified; otherwise maintain our own transaction ledger plus before/after checks.
- Respect protected/elevated processes and UAC integrity boundaries; do not bypass them.
- Detect app normalization (e.g., stripping double spaces) and report inability rather than repeatedly retrying.

**Minimum app test matrix:** Windows Notepad; Word desktop; Outlook classic/new or actual email app used; Chrome/Edge plain input and contenteditable field; one rich-text editor; an unsupported target; password field; app-switch mid-command. Record exact app/version, input adapter, and undo result.

## 9a. Live text is the source of truth

**Decision (October 17, 2026):** For “correct that”, the focused application's current text is authoritative, not Striper's copy. A copy kept by Striper breaks the moment the user types, moves the cursor, clicks elsewhere or edits something himself. Reading the live field also lets him correct text he typed or a letter he opened yesterday.

### The correction loop

1. **Read** the focused text field: its text (or the visible/recent window the API exposes), the selection and the caret position, together with the window/control identity.
2. **Plan:** send that text plus the spoken instruction to the LLM, and resolve the returned operations against the snapshot into concrete edits.
3. **Verify unchanged:** read the field again just before applying. If the text, selection or control changed while the LLM was working, do not apply the old plan: re-plan once against the new text, and refuse with a clear message if it changes again.
4. **Apply** a targeted edit to exactly the planned range, then read back and confirm the result; on mismatch, undo and report.

Session history (the ledger) is used only to understand “undo that”, “scratch that” and follow-ups such as “actually, Friday”; it does not decide what is currently in the document.

### Platform adapters and capabilities

- A platform trait exposes `read` (text, selection, caret, how much of the document is visible) and targeted `select range` / `replace selection` operations, plus a capability report. Shared planning and verification live in `striper-core` and are tested against a simulated field.
- **Windows first:** UI Automation (TextPattern/TextPattern2 for document text, selection and caret; ValuePattern for simple fields), never reading pixels. UIA TextPattern is read-mostly, so edits select the target range through UIA where supported and then use the existing verified paste/keystroke path. Word may later get its own adapter.
- **Linux/KDE:** AT-SPI (Text, EditableText) where the toolkit exposes it; otherwise the existing keystroke/clipboard path.
- **Fallbacks, in order:** full field text via accessibility → the selected text only (correct just the selection) → Striper's recent-dictation ledger (current Tier 1 behavior) → refuse. When a field cannot be read or safely edited, say so plainly (“I can't read this field; select the text you want corrected” or “This app doesn't let me edit safely”) rather than guessing.
- **Honest scope:** only promise whole-document edits where the whole document is actually readable. When an API exposes only a bounded window around the caret, corrections are limited to that window and the user is told so. Never send entire documents to a hosted model by default (§7): the context sent is bounded around the caret/selection and the target of the instruction.
- Password fields, secure desktop and elevated windows remain refused (§7, §9).

### Acceptance

- [ ] In Notepad and Word: type a sentence by hand, then “correct that, change Thursday to Friday” edits the typed text.
- [ ] Open yesterday's letter, place the caret in it, and correct a phrase in view.
- [ ] Type in the field while the LLM is thinking: Striper re-reads and re-plans or refuses; it never applies a stale edit.
- [ ] An app with no accessible text falls back to the selection or refuses with a clear message.

## 10. Wake-word engineering

“Correct that” is short and could appear in ordinary speech. Train a dedicated detector (not assumed to ship pre-trained), test it on the actual user's microphone/accent, and provide an alternate hotkey. Avoid two unrelated processes simultaneously opening the microphone in incompatible exclusive modes; a shared capture/dispatch service may be required.

- CPU-first wake detector + VAD, with optional per-user verification.
- Clear mic-on state; no stored background audio by default.
- Threshold tuning with recordings of similar phrases and background conversation.
- Pause detector while Handy is actively recording normal dictation if collision/false triggers occur; assess whether the wake phrase is useful during dictation vs after release.
- Distinct activation, confirmation, cancellation sounds; adjustable or muteable.
- Emergency stop: tray Pause + hotkey, and no edits when paused.

## 11. Hardware and inference choices

**Dad's PC:** NVIDIA GTX 1080, Handy/Whisper currently fast. Keep the working model and settings. The GPU has finite VRAM, so an extra local LLM may compete with Whisper; do **not** assume simultaneous GPU inference will be faster. Measure before adopting one.

**Default MVP:** Handy's existing local ASR, CPU wake listener, local SQLite dictionary, and a fast low-cost hosted command model through a configurable OpenAI-compatible endpoint. Send **text-only** commands/context, not raw microphone audio, unless explicitly opted in.

**Alternative:** local command model (CPU or GPU if performance acceptable), or a user-hosted LiteLLM endpoint. Implement both behind the same interface. Validate any proposed third-party free credits, billing, API format, rate limits, and privacy separately.

Performance targets to benchmark, not promises:

- Lane A dictionary overhead: target < 50 ms at p95 for a typical utterance.
- Wake detection: target < 500 ms after the phrase completes at p95.
- Lane B edit: target < 3 seconds p95 on a fast provider, excluding speech duration.
- No crashes, hangs, lost clipboard state, or focus theft in normal use.

## 12. Milestones and acceptance gates

### M0 — Reproduce baseline and inspect code

- [ ] Fork/clone/pin upstream; build on Linux for development and in CI on Windows 11 with matching Handy configuration.
- [ ] Document key source files/functions in the transcription, post-processing, insertion, and hotkey paths.
- [ ] Run controlled timing tests on the GTX 1080 and sample apps.
- [ ] **Gate:** cross-platform shared core compiles/tests on Linux and Windows; a reliable pre-insertion/commit interception design and viable Windows edit path are demonstrated.

### M1 — Two-lane proof of concept, hotkey command first

- [ ] Add bridge event(s) in the Handy fork; preserve Ctrl+Space behavior on Windows and the configured Linux shortcut.
- [ ] Implement explicit edit hotkey using Handy ASR or a coordinated capture path.
- [ ] Structured LLM command parsing with schema validation; initially one operation: replace exact text within the last committed dictation.
- [ ] Exact target, stale-focus, and text-mismatch checks; visible undo.
- [ ] **Demo:** dictate “Thursday”; say “change Thursday to Friday”; undo; confirm original text restored in Windows Notepad. Shared logic must run in a Linux test harness.

### M2 — Learned spelling and post-ASR correction

- [ ] SQLite vocabulary, deterministic correction engine, backup/export and migrations.
- [ ] Correct names during normal Lane A dictation before pasting.
- [ ] Learn/forget via voice with confirmation for ambiguous replacements.
- [ ] **Demo:** teach “Stephen”; restart; dictate name in context; confirm preferred spelling. Show that another person named Steven is not blindly changed.

### M3 — Wake phrase and conversational session

- [ ] Train/tune “correct that” model; integrate shared audio control or safe microphone arbitration.
- [ ] Add session state machine, timeout, “done,” cancel, follow-up reference and feedback.
- [ ] Measure false wakeups and missed wakeups on actual microphone.
- [ ] **Demo:** “correct that -> Thursday to Friday -> actually Saturday -> done” without extra keystrokes on Windows; demonstrate same core workflow in one supported Linux editor.

### M3a — Hands-free session and live-text corrections (Dad's workflow)

- [ ] Session state machine in `striper-core`: Off / Listening / Paused, standalone mute/unmute with pause checks, “new paragraph” formatting, “correct that” routing; unit-tested.
- [ ] App wiring: Ctrl+Space toggle mode, large Start/End dictation button, overlay/tray state, distinct sounds, pause-detected commits through the shared microphone and selected recognizer.
- [ ] Paused mode runs only the small local “unmute” listener; everything else is discarded; ending the session stops it.
- [ ] Live-text trait and verified loop (read → plan → verify unchanged → apply → read back), paragraph-only operations validated to change whitespace only.
- [ ] Windows UIA read adapter (TextPattern, ValuePattern, selection, caret) with fallbacks and clear refusals; Linux AT-SPI adapter afterwards.
- [ ] **Demo:** §5a and §9a acceptance lists on Windows 11, then on Dad's PC.

### M4 — Windows-first compatibility, Linux integration, safety and usability

- [ ] Implement/test Windows UIA and at least one rich-text/Word adapter as justified by results.
- [ ] Implement/test a supported Linux editing adapter (X11 and/or Wayland where permissions/API support it) and document unsupported environments.
- [ ] Build app capability detection and safe fallback; never use blind deletion.
- [ ] Test clipboard races, focus switches, external document changes, password fields, Undo.
- [ ] One-page shared settings UI, clear microphone/privacy toggle, Windows installer and Linux package, and first-run configuration.
- [ ] **Gate:** Dad can perform common dictation and correction tasks for a week without needing developer intervention.

### M5 — Polish and possible open-source release

- [ ] Improve preferences/scopes, correction UX, offline provider, telemetry opt-in, documentation.
- [ ] Add reproducible Windows/Linux builds, signed installers if possible, issue templates, license/attribution compliance, accessibility review.
- [ ] Invite opt-in tester feedback; upstream generic bridge fixes where appropriate.

## 13. Test plan / definition of done

Automated unit tests:

- [ ] Vocabulary normalization, names, punctuation, whitespace, casing, multiword phrases, Unicode.
- [ ] JSON-schema validation, malformed/adversarial command rejection, ambiguity clarification.
- [ ] Consecutive edits, undo, repeated command deduplication, stale target invalidation.
- [ ] No persistent rule created without user confirmation.

Interactive tests (Windows first, plus Linux integration):

- [ ] Normal Ctrl+Space remains as responsive and accurate as unmodified Handy.
- [ ] Edit recent text in Notepad and at least one real email/word-processing workflow.
- [ ] Change two spaces and punctuation without silent trimming where app permits.
- [ ] Correct a name, restart the app, retain preference, and remove preference by voice.
- [ ] Simulate disabled internet, bad API key, slow API, denied access, microphone disconnect, clipboard contention, and focus switch.
- [ ] Verify no change is applied in a stale/unrelated field and every successful assistant edit can be undone.
- [ ] Test without keyboard after initial setup; feedback must be understandable for nontechnical users.
- [ ] Run on KDE Plasma X11 and Wayland (when available), document compositor permission requirements, and validate safe fallback on unsupported fields.

**Release-ready MVP:** Reliable regular Handy dictation + exact recent-transcript editing + persistent corrections + safe undo + understandable fallback. Wake word and broad app support progress only once safety and latency pass measurement gates on Windows 11, with final microphone/application checks on Dad's PC. Linux results alone do not establish Windows readiness.

## 14. Risks and choices to settle through experiments

| Risk | Mitigation / decision |
|---|---|
| Handy doesn't expose a suitable text hook | Patch/fork at the existing post-ASR, pre-insertion stage; keep patch small and maintainable. |
| Phrase triggers during ordinary dictation | Tune threshold, suspend during push-to-talk, keep hotkey fallback. |
| Two processes fight over microphone or GPU | Share capture when possible; CPU wake model; serialize/reuse ASR resources. |
| No trustworthy cursor/text range in some apps | Restrict to last committed transcript or manual-copy fallback; label app unsupported. |
| Linux Wayland restricts focus access and injected input | Use supported portals/accessibility integrations where possible; capability-test each desktop, provide hotkey/manual paste fallback. |
| Windows CI build differs from Linux development machine | Native Windows CI builds and a real Windows 11 smoke-test machine; avoid assuming effortless Tauri cross-compilation. |
| AI misidentifies an edit target | Require expected-text match and unique range; ask to clarify instead of guessing. |
| Ambiguous name preferences cause incorrect replacements | Explicit confirmation and context-scoped dictionary, not global replace-all. |
| Hosted provider outages/costs/privacy | Fixed small context, fail-open normal dictation, optional local backend and spend cap. |
| Handy upstream changes break fork | Pin commit, CI build checks, minimal patch, documented merge plan. |

## 15. Suggested repository layout (Handy fork with shared core and platform adapters)

```text
Handy/                         # Our fork of cjpais/Handy
  PLAN.md
  README.md
  docs/
    handy-integration.md
    windows-app-matrix.md
    privacy-and-safety.md
    architecture-decisions/
  bridge/                    # Minimal Handy integration/patch
  editor-core/               # Transaction, dictionary, command schemas
  platform-adapters/
    windows/                  # UIA + tested app-specific editors
    linux/                    # X11/Wayland adapters + safe fallbacks
  listener/                  # Wake phrase + audio-session coordination
  inference/                 # Configurable LLM client and validation
  desktop-ui/                # Tray and preferences
  .github/workflows/          # Linux + native Windows CI builds
  tests/
    fixtures/
    integration/
```

This is a **proposed** layout, not a claim that these files exist upstream. An in-process-only design is acceptable if the M0 spike shows it is more reliable.

## 16. Immediate next actions

1. Fork and pin Handy; build on Linux and add native Windows CI builds. Inspect exact transcription -> post-process -> insertion control flow.
2. Implement the shared transaction bridge and platform-neutral edit operation contract; prove last-inserted-text replacement on Windows 11.
3. Prove safe undo and focus validation **before** adding a wake word or elaborate prompt.
4. Add SQLite named-entity corrections and confirm persistence/ambiguity handling.
5. Attach configurable fast-model command parsing, followed by wake-phrase interaction.
6. Build Dad's hands-free session (§5a) and the live-text correction loop with the Windows UIA adapter (§9a, M3a).
7. Test with Dad's actual Word/email apps, microphone, and GTX 1080; exercise the same shared logic on Linux (Plasma X11/Wayland) and document capabilities rather than assuming universality.

## 17. References (project/docs to confirm at pinned upstream commit)

- Handy official repository, README and CLI: https://github.com/cjpais/Handy
- Handy settings model (custom words, post-processing): https://github.com/cjpais/Handy/blob/main/src-tauri/src/settings.rs
- openWakeWord library and custom model training: https://github.com/dscripka/openWakeWord
- Microsoft: UI Automation TextPattern overview: https://learn.microsoft.com/en-us/dotnet/framework/ui-automation/ui-automation-textpattern-overview
- Microsoft: TextPattern is principally read-only / TSF context: https://github.com/MicrosoftDocs/win32/blob/docs/desktop-src/WinAuto/uiauto-about-text-and-textrange-patterns.md

---

**Bottom line:** Fork Handy and build a cross-platform shared core on Linux, with tested Windows and Linux editing adapters. Start with an authoritative transcript hook, a safe edit coordinator, and a persistent vocabulary. Prove those first. Then make the experience conversational through a wake phrase and an LLM that proposes edits—but never controls the desktop directly.