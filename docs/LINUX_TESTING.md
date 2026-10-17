# Linux development and Windows acceptance

Striper is a Windows-first program for Dad on Windows 11. Ben develops on a
KDE Plasma Linux desktop, and Linux remains supported. Linux testing helps
iteration; it does not replace Windows acceptance. A Windows 11 guest,
potentially through WinBoat, is the planned Windows test route until Dad's PC
is available. No Windows desktop behavior has been inferred from Linux results.

## Build policy

Routine pushes run the existing test and code-quality workflows. Installer
packaging and the full Nix build are manual. In GitHub Actions, choose **Main
Branch Build → Run workflow**, selecting `linux` (the default), `windows`, or
`both`. Build one installer when a batch of changes is ready to try. The
separate Build Test workflow still offers the wider architecture matrix.

For local development, install the dependencies in [BUILD.md](../BUILD.md), run
`bun install`, and use `bun run tauri dev`. Rust and frontend changes reuse their
local build caches. There is no need to package an installer for every edit.

## Desktop compatibility

Open **Vocabulary → Correction compatibility**. The checks are read-only: no
microphone capture, clipboard reads, key injection or provider calls. Refresh
after installing helpers. **Copy diagnostic report** copies a compact JSON
report with no document title, window ID, dictated text, or clipboard content.

| Session        | Foreground identification                | Correction input                                          | Clipboard                        |
| -------------- | ---------------------------------------- | --------------------------------------------------------- | -------------------------------- |
| Plasma X11     | X11 queries                              | xdotool when installed; existing fallback otherwise       | Native clipboard                 |
| Plasma Wayland | kdotool matching the Plasma version      | dotool or ydotool, with working device permissions/daemon | wl-copy and wl-paste recommended |
| GNOME Wayland  | No general foreground-window adapter yet | dotool or ydotool if configured                           | wl-copy and wl-paste recommended |
| Other Wayland  | Foreground identity may be unavailable   | wtype where supported, otherwise dotool/ydotool           | wl-copy and wl-paste recommended |

Finding a binary does not prove its permissions, compositor support, or daemon
readiness. The capability page reports detection, not a successful edit test.
`kdotool` identifies windows; it does not send correction keystrokes. `wtype` is
not selected on KDE or GNOME Wayland. A desktop with no observable window ID
cannot activate ambient correction. The explicit shortcut remains available,
with source-selection readback required by default.

KDE foreground queries have a 250 ms deadline. Idle wake detection samples
focus at most four times per second with a minimal window-ID query. Edit-time
checks remain uncached; losing a previously known window identity refuses
further editing actions.

## First manual pass

Use a disposable document in Kate or another plain-text editor; do not start
in a terminal, where Ctrl+C is an interrupt instead of a selection copy.

1. Copy a harmless piece of text to the clipboard so restoration is observable.
2. Hold Ctrl+Space and dictate “I'll see you on Thursday.” Release the keys.
3. Use the correction shortcut and say “change Thursday to Friday”. Confirm
   the word changes and the rest of the sentence stays intact.
4. Say “actually Saturday” through the correction shortcut, then “undo that”.
   Confirm the sentence returns to Friday.
5. Repeat using the optional voice listener: say “correct that, change Friday
   to Thursday”. For a standalone “correct that”, wait for “Listening for a
   correction” before giving the instruction. End with “done”.
6. Switch to another window during a correction. Confirm the edit is refused.
7. Move the caret or modify the source text yourself, then request a correction.
   Confirm mismatched source text is refused rather than replaced blindly.
8. Check that the clipboard's original contents were restored.
9. Teach “Striper” using the test in PLAN.md §0; restart, then check learned
   spelling, ordinary “striped shirt”, and forgetting the preference.

Record the desktop session, editor/version, selected input backend, and the
result of each step. Keep “Check before editing” enabled during initial tests.

## Testing in a VM

A guest can test its own UI, settings, vocabulary persistence, clipboard,
correction and undo. Start with a plain text editor and a CPU recognizer if
GPU passthrough is not configured. Check which system owns Ctrl+Space and
Ctrl+Alt+Space; rebind the guest shortcut if the host intercepts it. Microphone
passthrough and clipboard integration also need an explicit guest check.

A Linux guest does not establish Windows compatibility. A Windows guest can
test Windows text fields and adapters, but does not establish GTX 1080 timing,
GPU-driver behavior, or the real microphone's wake accuracy. Keep host and
guest results separate in the test notes.

## Windows 11 guest acceptance (including a possible WinBoat setup)

Use the Windows application **inside the Windows 11 guest**, installed from a
Windows artifact. A Linux build presented on the host does not exercise the
Windows adapters. In Main Branch Build, select `windows` for this installer;
the workflow's Linux default reflects the development environment, not product
priority. Reuse that installer through a manual test batch.

1. Confirm the guest receives microphone audio and the intended recording
   shortcuts. Check host/guest shortcut interception and clipboard integration
   before diagnosing the Windows app. Start with CPU recognition if the guest
   has no working GPU acceleration.
2. Install and launch as a normal user, select a microphone/model, and test
   ordinary dictation in Windows Notepad. Record Windows version, app commit,
   model, and guest configuration.
3. Enable the voice listener and test “correct that” without pressing a
   correction shortcut: both a same-breath instruction and a standalone wake
   followed by its listening acknowledgment. Test follow-ups and “done”. Keep
   the shortcut available as a fallback, not as the primary correction UX.
4. Run PLAN.md §0's remembered-name/restart check and the Thursday → Friday →
   Saturday → undo sequence. Check clipboard restoration, manual caret/text
   changes, focus switches and cancellation with “Check before editing” on.
5. Repeat in the email/word-processing applications Dad actually uses when
   available in the guest. A Notepad pass alone does not establish Word or
   browser-field compatibility. Record unsupported controls and clear fallback
   behavior rather than disabling verification to make a test pass.
6. Check tray launch/quit, restart persistence, readable/audio feedback and a
   voice-led correction session requiring no correction keystroke after setup.
7. Before calling it ready for Dad, repeat on his PC with his microphone and
   everyday applications. Guest results do not establish native GPU timing or
   his microphone's wake accuracy.

If no Windows guest is ready, continue shared-code fixes and Linux smoke tests,
but leave the Windows acceptance items open. The product priority remains
Windows 11 usability for Dad.
