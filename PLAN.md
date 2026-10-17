# Striper — Development Plan

**Status:** In implementation. See [Implementation status](#implementation-status) and [docs/AI_HANDOFF.md](docs/AI_HANDOFF.md)  
**Date:** October 17, 2026  
**Initial target:** Cross-platform fork developed primarily on Linux; first deployment Windows 11 with Handy + local Whisper and NVIDIA GTX 1080 (8 GB VRAM)  
**Audience:** A Dragon NaturallySpeaking user on Windows and a Linux-based developer who also wants the tool available on Linux  
**Application name:** Striper (S-T-R-I-P-E-R), named for the striped bass; not “striped” or “stripper.”

## Implementation status

_Last updated October 17, 2026. "Tested" means automated tests on Linux (and Windows CI for the core crates). "Needs Windows" means the code exists but has not run on Dad's Windows 11 machine, his microphone or the GTX 1080. Checkboxes in the sections below stay unticked until their acceptance gate has passed on real hardware._

| Area | State | Evidence |
|---|---|---|
| Fork, rebrand to Striper, Handy attribution kept | Done | `9bf8a10`; About page and README credit Handy; original icon kept at `assets/branding/striper-original-icon.png` |
| Native Windows CI build (unsigned NSIS/MSI artifact) | Done, needs first green run observed | `.github/workflows/main-build.yml` |
| Shared core crate (`crates/striper-core`): vocabulary, command schema, parser, ledger, verified edit planner, SQLite store | Done, tested | 86 unit tests on Linux and Windows CI |
| Platform crate (`crates/striper-platform`): foreground window + modifier state, Windows and X11 (KDE Wayland via kdotool if installed) | Done, Windows path type-checked only | Needs Windows |
| Lane A: learned vocabulary applied before paste, context-aware, per-app scope, ASR hints | Done, tested in core | Needs Windows |
| Lane A keeps working with no LLM | Done | Lane A never calls the LLM |
| Lane B: correction shortcut (Ctrl+Alt+Space) and `--toggle-correction`, shared mic/ASR path with Lane A | Done | Needs Windows |
| Lane B operations: replace, follow-up ("actually Saturday"), punctuation, spaces, insert, capitalize, delete, scratch that, undo, remember, forget, cancel, done | Done, tested in core | Needs Windows (Notepad, Word, email) |
| Local parser first, OpenAI-compatible LLM fallback with strict schema, timeout and back-off | Done | Needs a live endpoint |
| Verified, transactional, reversible edits (selection read-back, post-verify, undo on mismatch, clipboard restore, stale-focus check) | Done, tested against a simulated field | Needs Windows; read-back depends on each app allowing copy |
| Persistent memory only when the user says "remember", stored in SQLite with audit log | Done, tested | §0 acceptance test passes in core tests; needs a real restart on Windows |
| Vocabulary manager UI, import/export, recent corrections, overlay feedback | Done | Lint and build pass; not yet viewed on screen |
| Wake phrase "correct that" (CPU detector) | Not started | Hotkey is the fallback per M1 |
| Windows UI Automation / Word adapter | Not started | Keystroke + clipboard path first |
| Data migration from Handy's `com.pais.handy` app-data folder | Not started | Copy models/settings by hand for now |
| Updater signing keys for Striper | Not started | Update checks default off |

## 0. First acceptance test — Striper must remember its own name

**Problem:** Dictating “Striper” may yield “striped” or “stripper.” This is a small but useful end-to-end test of the promised correction and persistent vocabulary features.

1. During ordinary dictation, deliberately capture a misrecognized mention of the application's name, such as “I opened striped” or “launch stripper,” where the user really meant **Striper**.
2. Say “correct that; I meant Striper, S-T-R-I-P-E-R; remember that” (or an equivalent natural correction). Striper should identify the intended prior text, preview or safely apply the edit, and save the spelling as an explicit user-taught proper name.
3. Across later dictations and application restarts, recognize “Striper” in relevant contexts without making the user spell it again. Prefer a recognizer custom-word hint when available, with a validated post-transcription normalization fallback where necessary.
4. **Do not globally replace** the valid English words “striped” or “stripper” in unrelated sentences. Context matters; when intent is uncertain, ask or leave the transcript unchanged.
5. Confirm that undo/revert works, the saved preference can be reviewed or deleted, and the behavior is tested on Windows first and Linux afterward.

This requirement is part of the first milestone, not a cosmetic cleanup. It proves the ASR -> correction -> persistent memory -> safe insertion loop while preserving normal dictation.

## 1. Mission

Fork and extend the existing **Handy** dictation experience rather than replace it. Develop the shared code on Linux and build/test both Windows and Linux targets; prioritize Windows compatibility first because that is Dad's machine. Normal dictation should remain quick and familiar: hold **Ctrl+Space**, speak, release, and the words appear in the active application. Add a second, conversational lane activated by **“correct that”** (with a keyboard fallback). The user can then say what to change in ordinary English, and the system safely updates the relevant text.

A correction such as “My first name is Stephen, S-T-E-P-H-E-N; remember that” should persist across restarts. We should not have to retrain Whisper or pay for a Dragon upgrade.

**Design principle:** The speech recognizer transcribes; a command interpreter understands intent; a deterministic editor makes changes; a local dictionary remembers preferences. **The LLM must never receive unrestricted control of Windows.**

## 2. Goals and non-goals

### Must have

- [ ] Keep the current Handy **Ctrl+Space** dictation flow and its working GPU-accelerated Whisper setup.
- [ ] Support most ordinary Windows text fields, initially with explicit supported-app coverage.
- [ ] Activate correction mode by voice (“correct that”) without requiring rigid Dragon command syntax.
- [ ] Understand edits such as “change Thursday to Friday,” “replace that question mark with a period,” “put two spaces after the sentence,” “scratch that,” and “undo that.”
- [ ] Support follow-up commands in the same editing session: “Actually, make it Saturday.”
- [ ] Remember learned names, spellings, and explicit user preferences across sessions, beginning with the app's own name, **Striper**.
- [ ] Make edits reversible and refuse unsafe/ambiguous edits rather than guessing.
- [ ] Offer an **OpenAI-compatible** LLM endpoint (local or hosted); keep normal dictation working if it is unavailable.
- [ ] Maintain one cross-platform core and platform-specific text-editing adapters for Windows and Linux.
- [ ] Support a Linux-first development workflow with automated Windows builds and tests; Dad's Windows 11 machine is the first acceptance environment.
- [ ] Ship a simple Windows installer and Linux package/tray app with accessible visual/audio status and minimal setup.

### Nice to have after MVP

- [ ] Optional local LLM execution, fully offline mode, voice-based vocabulary review.
- [ ] Select phrase / select sentence / replace selected text by speech.
- [ ] Word-specific formatting, number formatting, punctuation styles, per-application profiles.
- [ ] Export/import dictionary and profiles; advanced Linux desktop adapters (X11 and Wayland) and additional application-specific integrations.

### Not in MVP

- Full Dragon feature parity, voice macros for arbitrary desktop automation, browser navigation, or filesystem control.
- Blind editing of entire documents or unbounded clipboard history.
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

Allowed operations: `replace_text`, `replace_punctuation`, `insert_text`, `delete_recent_dictation`, `undo_last_edit`, `remember_spelling`, `forget_spelling`, `clarify`, `cancel`, `finish`.

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

**Release-ready MVP:** Reliable regular Handy dictation + exact recent-transcript editing + persistent corrections + safe undo + understandable fallback. Wake word and broad app support progress only once safety and latency pass measurement gates.

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
6. Test with Dad's actual Word/email apps, microphone, and GTX 1080; exercise the same shared logic on Linux (Plasma X11/Wayland) and document capabilities rather than assuming universality.

## 17. References (project/docs to confirm at pinned upstream commit)

- Handy official repository, README and CLI: https://github.com/cjpais/Handy
- Handy settings model (custom words, post-processing): https://github.com/cjpais/Handy/blob/main/src-tauri/src/settings.rs
- openWakeWord library and custom model training: https://github.com/dscripka/openWakeWord
- Microsoft: UI Automation TextPattern overview: https://learn.microsoft.com/en-us/dotnet/framework/ui-automation/ui-automation-textpattern-overview
- Microsoft: TextPattern is principally read-only / TSF context: https://github.com/MicrosoftDocs/win32/blob/docs/desktop-src/WinAuto/uiauto-about-text-and-textrange-patterns.md

---

**Bottom line:** Fork Handy and build a cross-platform shared core on Linux, with tested Windows and Linux editing adapters. Start with an authoritative transcript hook, a safe edit coordinator, and a persistent vocabulary. Prove those first. Then make the experience conversational through a wake phrase and an LLM that proposes edits—but never controls the desktop directly.