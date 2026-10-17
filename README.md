# Striper

<p align="center">
  <img src="assets/branding/striper-original-icon.png" alt="Striper logo: a leaping striped bass above a sound waveform" width="200">
</p>

**AI-assisted, offline-first dictation with a conversational correction lane.**

Striper (S-T-R-I-P-E-R, named after the striped bass) is a cross-platform desktop dictation app. Hold a shortcut, speak, release, and your words appear in the active application — transcribed locally on your own computer. On top of that familiar flow, Striper is adding a second, conversational lane: say **"correct that"** and describe the fix in plain English ("change Thursday to Friday", "I meant Striper, S-T-R-I-P-E-R; remember that"), and Striper safely edits the text and remembers names and spellings you teach it.

Striper is Windows-first, built primarily for Dad on Windows 11, and also supports Linux. Development happens on a Linux desktop; a Windows 11 VM (potentially through WinBoat) is the planned Windows test environment until his PC is available. See [PLAN.md](PLAN.md) for the roadmap and the first acceptance test.

> **Forked from [Handy](https://github.com/cjpais/Handy).** Striper is built on Handy, the free and open source speech-to-text app by [CJ Pais](https://github.com/cjpais) and contributors. All of Handy's local transcription, model management, and paste pipeline comes from upstream. See [Attribution](#attribution).

## How It Works

1. **Press** a configurable keyboard shortcut: hold it to record and release to stop, or tap it to toggle recording on and off (Hold-only and Toggle-only modes are also available)
2. **Speak** your words while the shortcut is active
3. **Release** and Striper processes your speech locally
4. **Get** your transcribed text pasted directly into whatever app you're using

The process is entirely local:

- Silence is filtered using VAD (Voice Activity Detection) with Silero
- Transcription uses your choice of models:
  - **Whisper models** (Small/Medium/Turbo/Large) with GPU acceleration when available
  - **Parakeet** - Fast transcription with CPU or GPU execution through the current GGUF engine; language support depends on the variant
- Works on Windows and Linux (macOS builds from source)

### Voice correction

On Windows x64 or Linux x64, open **Vocabulary → Voice correction**, download the local listener files, and enable **Listen for “correct that”**. This is an explicit background-microphone opt-in; audio is processed locally on the CPU and background audio is never saved or uploaded.

After dictating with Ctrl+Space, release the keys and say **“correct that, change Thursday to Friday.”** You can also say **“correct that”**, wait for **“Listening for a correction”**, then give the instruction. Continue with **“actually Saturday”**, **“undo that”**, or **“done”**. Follow-up listening ends after 30 seconds or a focus change. Ctrl+Alt+Space remains the correction shortcut fallback.

The CPU listener pauses during ordinary dictation, so the words “correct that” in dictated prose remain ordinary text. It shares the same microphone recorder. Candidate wake phrases are independently checked by the selected recognizer before editing; the local parser handles familiar instructions and a configured AI provider handles the rest. Learned spellings stay in the local dictionary. An LLM never receives desktop or keystroke control, and normal dictation keeps working without it.

Use **Vocabulary → Correction compatibility** to check the desktop and helper tools. See [Linux testing](docs/LINUX_TESTING.md) for a desktop/VM test sequence. Installer packaging is manual so routine commits do not launch long native installer builds.

Voice activation still needs testing with the actual user's microphone and applications. See [the handoff](docs/AI_HANDOFF.md) for the current verification status.

### Choosing a model and hardware

For English dictation, start with **Parakeet Unified EN 0.6B** (the catalog's default Q8_0 download). It is not CPU-only in this version of Striper: transcription uses `transcribe-cpp`, with Vulkan GPU support in the Linux and Windows x64 builds. No separate “CUDA model” is needed. A GPU-capable model can also run on CPU; speed depends on your hardware and audio, and has not been benchmarked on your machine.

On an NVIDIA desktop, open **Advanced → Hardware Acceleration** and select the detected GPU (the menu includes its backend), or use **Auto** for GPU preference with CPU fallback. Load the model and check **Loaded engine** in the model picker or Models page: it reports the worker's actual CPU/GPU, backend and device. A detected GPU or saved preference alone does not prove GPU execution. The status refreshes on model load/unload and when the app regains focus. The separate voice-command listener runs on CPU regardless of the transcription setting.

**Whisper Large v3 Turbo** is another useful model to compare on an RTX 4090; this is its catalog name, not “Whisper Medium Turbo.” Use the default Q8_0 download first. Compare the same short dictations for accuracy and delay before choosing a default for Dad's Windows PC; results on the 4090 do not establish performance on his GPU.

In the footer model switcher, the active model shows **On GPU · Vulkan** or **On CPU** once it has loaded. Search model names in setup, **Models**, or the footer switcher. Full model lists are alphabetical (the small featured onboarding recommendations keep their curated order); search accepts multiple words such as `whisper turbo` or `parakeet unified`. Provider/AI model menus are also alphabetical and retain their existing type-to-search behavior.

## Quick Start

### Installation

Striper does not publish signed releases yet. Use an installer from the [releases page](https://github.com/BenStein1/Striper/releases), when available, or a successful **Main Branch Build** run as described below.

#### Windows

Download the Windows artifact from a successful Windows build, extract the ZIP, and run the `Striper_*_x64-setup.exe` (NSIS) or `.msi` installer. Builds are unsigned, so SmartScreen may require **More info → Run anyway**. Launch Striper, select a microphone and model, and configure shortcuts. Windows 11 is the primary user target. Use a Windows 11 guest for early testing from Linux; final acceptance still includes Dad's own PC and applications.

#### Kubuntu / Ubuntu / Debian (.deb)

These instructions cover a normal desktop installation, including a Linux VM. You do **not** need Rust, Bun, or the source code to install a downloaded package.

**1. Check your system.** Open Konsole (or another terminal) and run:

```bash
cat /etc/os-release
dpkg --print-architecture
printf 'Session: %s; desktop: %s\n' "$XDG_SESSION_TYPE" "$XDG_CURRENT_DESKTOP"
```

The current Linux installer is **amd64 / x86_64**, built on **Ubuntu 24.04**. Kubuntu uses Ubuntu packages. Other Debian-family releases must provide compatible system libraries; a `.deb` extension alone does not establish compatibility. If APT reports unavailable dependencies or startup reports a newer `GLIBC` requirement, use a compatible Ubuntu/Kubuntu installation or [build on your own distribution](BUILD.md). Do not replace system libraries manually. An ARM64 guest cannot use this installer.

**2. Download the Linux installer.**

1. Open [Main Branch Build in GitHub Actions](https://github.com/BenStein1/Striper/actions/workflows/main-build.yml).
2. Open a **successful run that includes the Linux job**, and check its commit/date. Routine test runs do not contain installers. A Windows-only run does not contain the Linux package.
3. Under **Artifacts**, download `striper-linux-ubuntu-24.04-x86_64-unknown-linux-gnu`. GitHub requires you to sign in to download workflow artifacts; artifacts expire after 30 days.
4. Extract the downloaded ZIP using Dolphin/Ark. Inside the extracted folders, find the **`.deb`** file (usually under `deb/`). The ZIP itself is not the installer. Use the `.deb` for this guide, rather than the AppImage.

If no usable artifact exists, a repository maintainer can select **Run workflow → platform: linux → Run workflow** on that page. Packaging is manual and can take a long time; reuse an existing installer until a new batch is ready to test. A successful installer represents its displayed commit, not necessarily the current source on `main`.

**3. Install with APT.** In Dolphin, open the folder containing the extracted `.deb`, then use **Open Terminal Here**. Run:

```bash
sudo apt update
ls ./*.deb
```

Use the exact filename printed above in place of `DOWNLOADED_FILE.deb`:

```bash
sudo apt install ./DOWNLOADED_FILE.deb
```

Keep the leading `./` and quote the path if it contains spaces. APT installs the package's declared runtime dependencies, including GTK layer shell and OpenBLAS; developer packages are unnecessary for a downloaded installer. If you already installed with `dpkg -i` and it left missing dependencies, run `sudo apt --fix-broken install`, then retry the APT command. If a dependency is unavailable for your release, stop and check compatibility rather than forcing the installation.

**4. Set up text input for your desktop session.** Use the session reported in step 1:

| Session            | Setup                                                                                                                                                                                                               |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Plasma **X11**     | Run `sudo apt install xdotool`. This is the simplest first-test path when your login screen offers a Plasma X11 session.                                                                                            |
| Plasma **Wayland** | Run `sudo apt install wl-clipboard`, then configure a working `dotool` or `ydotool` input backend and a Plasma-compatible `kdotool` for foreground-window detection. See [Wayland helpers](#wayland-helpers) below. |

Install the helpers **inside the guest** when testing in a VM. They control the guest desktop. A helper that works on the host does not configure the guest.

**5. Launch and complete first-run setup.**

1. Open **Striper** from the application launcher, or run `striper` in Konsole to see startup errors. If it is already running, launching it again opens its settings; check the system tray too.
2. Select your microphone and download/select a transcription model. Installer downloads do not include all speech models. Confirm the microphone works in the desktop's audio settings; a VM also needs microphone passthrough. Start with CPU transcription in a VM without GPU passthrough. On an NVIDIA desktop, keep the existing working GPU driver; installing Striper does not require replacing it.
3. Check the dictation shortcut (default **Ctrl+Space**) in Settings. If Plasma intercepts it or it does not register under Wayland, use the [desktop shortcut instructions](#linux-notes) below. A desktop command using `striper --toggle-transcription` starts recording on one press and stops on the next; it is not hold-to-talk. Avoid binding the same key through both Plasma and Striper.
4. Keep **Advanced → Overlay Position → None** during initial Linux tests so the recording overlay does not take focus from the editor.
5. Open **Kate**, click in a disposable document, and dictate a sentence. Confirm it pastes into Kate before testing correction. Do not use Konsole as the first correction target: Ctrl+C has a different meaning there.
6. Open **Vocabulary → Correction compatibility**. Refresh after installing helpers. Detection alone does not verify device permissions, daemon readiness, or successful editing; test in Kate as well. **Copy diagnostic report** provides a report you can share when troubleshooting.
7. For voice commands, open **Vocabulary → Voice correction**, download the local listener files (about 55 MB), and enable **Listen for “correct that”**. After dictating, release the dictation shortcut and say **“correct that, change Thursday to Friday.”** If you say only **“correct that”**, wait for **“Listening for a correction”** before giving the instruction. **Ctrl+Alt+Space** remains the correction shortcut fallback, rather than a requirement for voice correction.

Follow [Linux testing](docs/LINUX_TESTING.md) for correction, undo, focus changes, clipboard restoration, and VM checks. Keep **Check before editing** enabled during the first tests.

**Updating or removing Striper.** Quit the running app using its tray menu before installing a newer `.deb` with the same APT command, then launch it again. A package upgrade keeps your user settings and downloaded models. To remove the installed package:

```bash
sudo apt remove striper
```

Removing the package leaves user data in place. To find your data and logs, use Striper's **About → App Data Directory** entry. For startup/rendering errors, see [Linux startup troubleshooting](#linux-startup-crashes-or-instability).

### Development Setup

For detailed build instructions including platform-specific requirements, see [BUILD.md](BUILD.md).

## Usage

### Debug Mode

Striper includes an advanced debug mode for development and troubleshooting. Access it by pressing:

- **macOS**: `Cmd+Shift+D`
- **Windows/Linux**: `Ctrl+Shift+D`

### CLI Parameters

Striper supports command-line flags for controlling a running instance and customizing startup behavior. These work on all platforms (macOS, Windows, Linux). Largely this is a beta feature.

**Remote control flags** (sent to an already-running instance via the single-instance plugin):

```bash
striper --toggle-transcription    # Toggle recording on/off
striper --toggle-post-process     # Toggle recording with post-processing on/off
striper --toggle-correction       # Toggle explicit correction recording on/off
striper --cancel                  # Cancel the current operation
```

**Startup flags:**

```bash
striper --start-hidden            # Start without showing the main window
striper --no-tray                 # Start without the system tray icon
striper --debug                   # Enable debug mode with verbose logging
striper --help                    # Show all available flags
```

Flags can be combined for autostart scenarios:

```bash
striper --start-hidden --no-tray
```

> **macOS tip:** When Striper is installed as an app bundle, invoke the binary directly:
>
> ```bash
> /Applications/Striper.app/Contents/MacOS/Striper --toggle-transcription
> ```

## Known Issues & Current Limitations

This project is actively being developed and has some [known issues](https://github.com/BenStein1/Striper/issues). We believe in transparency about the current state:

### Bluetooth Headset Microphones (macOS)

Using a Bluetooth headset microphone on macOS may temporarily reduce playback quality or volume while recording because Bluetooth switches to bidirectional audio. Keep your headphones as the output device and select your Mac's built-in or an external microphone in Striper to avoid this.

### fn and Globe Key Shortcuts (macOS)

Shortcuts that include the `fn` (Globe) key **only work on Apple keyboards** — your Mac's built-in keyboard or an Apple external keyboard. They will never trigger on a third-party keyboard, even while it is connected to the same Mac.

This is a hardware limitation rather than a Striper bug. `fn` is not part of the standard USB HID keyboard specification: Apple reports it through a vendor-specific usage that macOS honors only from Apple devices, while third-party keyboards handle their `Fn` key entirely in firmware and send nothing to the computer. There is no event for Striper to listen for.

If you switch between a MacBook keyboard and an external one, pick a shortcut built from standard modifiers (`ctrl`, `option`, `shift`, `command`) or a regular key instead.

### Linux Notes

For package installation and a first test on Kubuntu, start with [Kubuntu / Ubuntu / Debian (.deb)](#kubuntu--ubuntu--debian-deb).

#### Wayland helpers

Wayland needs separate support for input, clipboard access, and foreground-window detection. Striper does **not** select `wtype` on KDE Plasma or GNOME. Installing `wtype` alone will not configure correction on those desktops.

| Session                   | Correction input                                        | Foreground-window detection                            | Clipboard                                                |
| ------------------------- | ------------------------------------------------------- | ------------------------------------------------------ | -------------------------------------------------------- |
| X11                       | `xdotool` (`sudo apt install xdotool`)                  | Native X11 queries                                     | Native clipboard                                         |
| Plasma Wayland            | `dotool` or `ydotool`, configured for your user         | `kdotool` matching your Plasma version                 | `wl-copy` / `wl-paste` (`sudo apt install wl-clipboard`) |
| GNOME Wayland             | `dotool` or `ydotool`, configured for your user         | No general adapter yet; ambient correction unavailable | `wl-copy` / `wl-paste`                                   |
| Other Wayland compositors | `wtype` where supported, otherwise `dotool` / `ydotool` | May be unavailable; check compatibility in Striper     | `wl-copy` / `wl-paste` recommended                       |

On **Plasma Wayland**:

1. Install clipboard tools with `sudo apt install wl-clipboard`.
2. Check which input packages your configured repositories offer: `apt-cache policy dotool ydotool ydotoold`. Install one available backend and follow its upstream/distro setup: [dotool](https://sr.ht/~geb/dotool/) or [ydotool](https://github.com/ReimuNotMoe/ydotool). For releases offering both ydotool packages, the installation command is `sudo apt install ydotool ydotoold`. Releases differ in daemon/service packaging, device permissions, and socket configuration, so do not assume a universal `systemctl` command. The chosen tool must work as your normal desktop user, with its required input-device access and daemon running. Do not run Striper with `sudo`.
3. Check your Plasma version in **System Settings → About this System** (or run `plasmashell --version`). Install [kdotool](https://github.com/jinliu/kdotool) using its upstream release/build instructions. **Version 0.3.0 and later target Plasma 6; Plasma 5 needs an older compatible release.** It must be on the desktop session's `PATH`. `kdotool` identifies windows; it does not send keystrokes.
4. Restart Striper if its environment changed, then refresh **Vocabulary → Correction compatibility**. Running `kdotool getactivewindow` in Konsole should return the current foreground window ID (normally Konsole's); this checks foreground queries, not input injection. Test an actual paste and correction in Kate before enabling the ambient listener.

Without a working foreground-window ID, ambient voice correction cannot activate. The explicit correction shortcut remains available, with source-selection readback required by default. If helpers are not yet configured and your login screen offers X11, sign out and select a Plasma X11 session for the initial test, then install `xdotool` there. See [Linux testing](docs/LINUX_TESTING.md) for the compatibility limits.

##### Ubuntu 25.10 / Plasma 6: one-time Wayland setup

For Ubuntu/Kubuntu releases that ship `ydotool` **0.1.8** (no `dotool` or `kdotool` packages), run the setup script from the clone as your normal user. It needs Rust (`cargo`) to build `kdotool`, and asks for your sudo password:

```bash
cd ~/Projects/Striper && git pull && scripts/linux/setup-plasma-wayland.sh
```

It installs `wl-clipboard`, `ydotool` and `ydotoold`, adds you to the `input` group so Striper's shortcuts can read the keyboard (log out and back in once if it says so), loads `uinput` at boot, grants the logged-in desktop user access to `/dev/uinput` through a udev `uaccess` rule, runs `ydotoold` as a systemd user service (`striper-ydotoold`, started with your Plasma session), and builds `kdotool` v0.3.0 into `/usr/local/bin` so apps started from the launcher find it. It ends by checking that the daemon is up and that `kdotool getactivewindow` answers. Note that any program running as you can inject input once `/dev/uinput` is accessible; that is inherent to ydotool. ydotool 0.1.8's daemon must run as your user (its socket is `/tmp/.ydotool_socket`, mode 0600), so do not start it with `sudo`.

To undo it: `systemctl --user disable --now striper-ydotoold`, then `sudo rm /etc/udev/rules.d/60-striper-uinput.rules /etc/modules-load.d/striper-uinput.conf /usr/local/bin/kdotool`.

Then run Striper from the clone (or install a `.deb` as above):

```bash
cd ~/Projects/Striper && env -u LD_LIBRARY_PATH -u LD_PRELOAD bun run tauri dev
```

If a terminal has inherited third-party libraries (for example Topaz's Qt libraries), check Plasma with `env -u LD_LIBRARY_PATH -u LD_PRELOAD plasmashell --version`. Refresh **Vocabulary → Correction compatibility** and test in Kate. Configure the Plasma dictation shortcut using `striper --toggle-transcription` for an installed package; when running only a development build, use that build's absolute executable path instead (usually `~/Projects/Striper/src-tauri/target/debug/striper`, with `~` expanded to your home directory).

**Other Notes:**

- **Runtime library dependency (`libgtk-layer-shell.so.0`)**:
  - Striper links `gtk-layer-shell` on Linux. If startup fails with `error while loading shared libraries: libgtk-layer-shell.so.0`, install the runtime package for your distro:

    | Distro        | Package to install    | Example command                        |
    | ------------- | --------------------- | -------------------------------------- |
    | Ubuntu/Debian | `libgtk-layer-shell0` | `sudo apt install libgtk-layer-shell0` |
    | Fedora/RHEL   | `gtk-layer-shell`     | `sudo dnf install gtk-layer-shell`     |
    | Arch Linux    | `gtk-layer-shell`     | `sudo pacman -S gtk-layer-shell`       |

  - For building from source on Ubuntu/Debian, you may also need `libgtk-layer-shell-dev`.

- The recording overlay is disabled by default on Linux (`Overlay Position: None`) because certain compositors treat it as the active window. When the overlay is visible it can steal focus, which prevents Striper from pasting back into the application that triggered transcription. If you enable the overlay anyway, be aware that clipboard-based pasting might fail or end up in the wrong window.
- Striper disables the WebKit DMA-BUF renderer by default on Linux, but preserves explicit `WEBKIT_DISABLE_DMABUF_RENDERER` values. For rendering workarounds, see [Troubleshooting → Linux Startup Crashes or Instability](#linux-startup-crashes-or-instability).
- If Striper fails to start reliably on Linux, see [Troubleshooting → Linux Startup Crashes or Instability](#linux-startup-crashes-or-instability).
- **Global keyboard shortcuts (Wayland):** On Wayland, system-level shortcuts must be configured through your desktop environment or window manager. Use the [CLI flags](#cli-parameters) as the command for your custom shortcut.

  **GNOME:**
  1. Open **Settings > Keyboard > Keyboard Shortcuts > Custom Shortcuts**
  2. Click the **+** button to add a new shortcut
  3. Set the **Name** to `Toggle Striper Transcription`
  4. Set the **Command** to `striper --toggle-transcription`
  5. Click **Set Shortcut** and press your desired key combination (e.g., `Super+O`)

  **KDE Plasma:**
  1. Open **System Settings → Shortcuts** (under **Keyboard** on some versions).
  2. Add a command or script shortcut named `Toggle Striper Transcription`, with command `striper --toggle-transcription`. In Plasma 5's **Custom Shortcuts** page, use **Edit → New → Global Shortcut → Command/URL**, then set the **Action** and **Trigger** tabs.
  3. Assign a free key combination. Each press toggles recording, so press it again to finish.
  4. Optionally add `striper --toggle-correction` on another shortcut as the explicit correction fallback. The voice listener is enabled separately in **Vocabulary → Voice correction**.

  **Sway / i3:**

  Add to your config file (`~/.config/sway/config` or `~/.config/i3/config`):

  ```ini
  bindsym $mod+o exec striper --toggle-transcription
  ```

  **Hyprland:**

  Add to your config file (`~/.config/hypr/hyprland.conf`):

  ```ini
  bind = $mainMod, O, exec, striper --toggle-transcription
  ```

- You can also trigger Striper externally via Unix signals or the CLI flags, which lets Wayland window managers or other hotkey daemons keep ownership of keybindings:

  | Action                                    | Trigger                                                      |
  | ----------------------------------------- | ------------------------------------------------------------ |
  | Toggle transcription                      | `pkill -USR2 -n striper` or `striper --toggle-transcription` |
  | Toggle transcription with post-processing | `striper --toggle-post-process`                              |

  Example Sway config:

  ```ini
  bindsym $mod+o exec pkill -USR2 -n striper
  bindsym $mod+p exec striper --toggle-post-process
  ```

  `pkill` here simply delivers the signal—it does not terminate the process.

  > **Behavior change:** older releases also accepted `SIGUSR1` for toggling transcription with post-processing. WebKitGTK — the webview engine embedded in Striper on Linux — uses SIGUSR1 internally to coordinate JavaScript garbage collection, so listening for it caused phantom recordings and interrupted dictations every few minutes ([Handy #1660](https://github.com/cjpais/Handy/issues/1660)). Striper no longer listens for SIGUSR1 on Linux; the post-processing toggle is still available via `striper --toggle-post-process`. **Remove any `pkill -USR1` bindings**: the signal is now delivered straight to WebKit's internal handler and can crash the app.

**Overlay & Pasting Issues (Linux):**

- The recording overlay window can interfere with pasting transcribed text into target applications on Linux (X11)
- **Solution:** Open **Settings > Advanced** and set **"Overlay Position"** to **"None"** to disable the overlay
- Enable **"Audio Feedback"** (also in Advanced) if you still want audible confirmation of recording state
- Users who upgrade from older versions or import settings from other platforms may need to manually apply this change

## Troubleshooting

### Previous Clipboard Content Is Pasted Instead of the Transcription

If the transcription is correct in **History** but Striper inserts text you copied earlier, see [upstream Handy issue #502](https://github.com/cjpais/Handy/issues/502). With the standard clipboard paste method, Striper restores your previous clipboard after a fixed delay. Under load, the receiving application may read the clipboard only after that restoration.

1. Open Striper's settings window and press `Cmd+Shift+D` (macOS) or `Ctrl+Shift+D` (Windows/Linux) to reveal **Debug**.
2. On **macOS and Windows**, try **Reliable Paste (Beta)** in Debug with a clipboard paste method selected. It uses clipboard read notifications to delay restoration instead of relying on the standard fixed delay. Test it in the application where the problem occurs; it is still experimental.
3. If Reliable Paste is disabled or unavailable, increase **Paste Delay (After)** in Debug and test again. This controls the wait before restoring your previous clipboard. **Paste Delay (Before)** controls the wait before sending the paste keystroke and addresses a different part of the operation. These delay settings apply to the standard paste path, not Reliable Paste.

If the problem persists, add your Striper version, operating system, receiving application, paste method, Reliable Paste setting, and before/after delays to the existing upstream issue or a new [Striper issue](https://github.com/BenStein1/Striper/issues). Redact private dictated text before sharing logs.

### Manual Model Installation (For Proxy Users or Network Restrictions)

If you're behind a proxy, firewall, or in a restricted network environment where Striper cannot download models automatically, you can manually download and install them. The URLs are publicly accessible from any browser.

#### Step 1: Find Your App Data Directory

1. Open Striper settings
2. Navigate to the **About** section
3. Copy the "App Data Directory" path shown there, or use the shortcuts:
   - **macOS**: `Cmd+Shift+D` to open debug menu
   - **Windows/Linux**: `Ctrl+Shift+D` to open debug menu

The typical paths are:

- **macOS**: `~/Library/Application Support/com.benstein1.striper/`
- **Windows**: `C:\Users\{username}\AppData\Roaming\com.benstein1.striper\`
- **Linux**: `~/.config/com.benstein1.striper/`

#### Step 2: Create Models Directory

Inside your app data directory, create a `models` folder if it doesn't already exist:

```bash
# macOS/Linux
mkdir -p ~/Library/Application\ Support/com.benstein1.striper/models

# Windows (PowerShell)
New-Item -ItemType Directory -Force -Path "$env:APPDATA\com.benstein1.striper\models"
```

#### Step 3: Download Model Files

Download the models you want from below

**Whisper Models (single .bin files):**

- Small (487 MB): `https://blob.handy.computer/ggml-small.bin`
- Medium (492 MB): `https://blob.handy.computer/whisper-medium-q4_1.bin`
- Turbo (1600 MB): `https://blob.handy.computer/ggml-large-v3-turbo.bin`
- Large (1100 MB): `https://blob.handy.computer/ggml-large-v3-q5_0.bin`

**Parakeet Unified EN 0.6B (single `.gguf` file, recommended):**

- Q8_0 (731 MB): `https://huggingface.co/handy-computer/parakeet-unified-en-0.6b-gguf/resolve/main/parakeet-unified-en-0.6b-Q8_0.gguf`

#### Step 4: Install Models

**For Whisper Models (.bin files):**

Simply place the `.bin` file directly into the `models` directory:

```
{app_data_dir}/models/
├── ggml-small.bin
├── whisper-medium-q4_1.bin
├── ggml-large-v3-turbo.bin
└── ggml-large-v3-q5_0.bin
```

**For GGUF Models (.gguf files):**

Place the `.gguf` file directly into the `models` directory, exactly like the Whisper `.bin` files above. Striper also picks up models already present in the shared Hugging Face cache (`~/.cache/huggingface/hub`), so a copy downloaded by another tool works without being moved.

**Important Notes:**

- Do not rename the `.bin` or `.gguf` files—use the exact filenames from the download URLs
- After placing the files, restart Striper to detect the new models

#### Step 5: Verify Installation

1. Restart Striper
2. Open Settings → Models
3. Your manually installed models should now appear as "Downloaded"
4. Select the model you want to use and test transcription

### Custom Whisper Models

Striper can auto-discover custom Whisper GGML models placed in the `models` directory. This is useful for users who want to use fine-tuned or community models not included in the default model list.

**How to use:**

1. Obtain a Whisper model in GGML `.bin` format (e.g., from [Hugging Face](https://huggingface.co/models?search=whisper%20ggml))
2. Place the `.bin` file in your `models` directory (see paths above)
3. Restart Striper to discover the new model
4. The model will appear in the "Custom Models" section of the Models settings page

**Important:**

- Community models are user-provided and may not receive troubleshooting assistance
- The model must be a valid Whisper GGML format (`.bin` file)
- Model name is derived from the filename (e.g., `my-custom-model.bin` → "My Custom Model")

### Linux Startup Crashes or Instability

If Striper fails to start reliably on Linux — for example, it crashes shortly after launch, never shows its window, or reports a Wayland protocol error — try the steps below in order.

**1. Install (or reinstall) `gtk-layer-shell`**

Striper uses `gtk-layer-shell` for its recording overlay and links against it at runtime. A missing or broken installation is the most common cause of startup failures and can manifest as a crash or a hang well before any window is shown. Make sure the runtime package is installed for your distro:

| Distro        | Package to install    | Example command                        |
| ------------- | --------------------- | -------------------------------------- |
| Ubuntu/Debian | `libgtk-layer-shell0` | `sudo apt install libgtk-layer-shell0` |
| Fedora/RHEL   | `gtk-layer-shell`     | `sudo dnf install gtk-layer-shell`     |
| Arch Linux    | `gtk-layer-shell`     | `sudo pacman -S gtk-layer-shell`       |

If it is already installed and you still see startup problems, try reinstalling it (e.g. `sudo pacman -S gtk-layer-shell` again) in case the library files were corrupted by a partial upgrade.

**2. Disable the GTK layer shell overlay (`HANDY_NO_GTK_LAYER_SHELL`)**

If installing the library does not help, you can skip `gtk-layer-shell` initialization entirely as a workaround. On some compositors (notably KDE Plasma under Wayland) it has been reported to interact poorly with the recording overlay. With this variable set, the overlay falls back to a regular always-on-top window:

```bash
HANDY_NO_GTK_LAYER_SHELL=1 striper
```

**3. Configure WebKit DMA-BUF rendering (`WEBKIT_DISABLE_DMABUF_RENDERER`)**

Striper disables the WebKitGTK DMA-BUF renderer by default on Linux because it can cause blank windows or crashes on some GPU/driver combinations. Explicit environment-variable values are preserved, including in the Nix package. To explicitly keep the renderer disabled:

```bash
WEBKIT_DISABLE_DMABUF_RENDERER=1 striper
```

Some users on WebKitGTK 2.54 report a flickering or partially drawn recording overlay with the renderer disabled ([Handy #2166](https://github.com/cjpais/Handy/issues/2166)). If affected, try opting out of the default workaround:

```bash
WEBKIT_DISABLE_DMABUF_RENDERER=0 striper
```

This allows WebKit to use the renderer when supported, but can reintroduce the GPU/driver-specific crashes described above. Remove the override or use `=1` if that happens.

Fully quit Striper, including its tray instance, before testing either value. Running a second instance with `--toggle-transcription` does not change the running process's environment.

**Making a workaround permanent**

Set the variable in the environment of the launcher that actually starts Striper. Shell profile exports may not apply to desktop-menu or autostart launches. If you launch Striper from a `.desktop` file, you can prefix the `Exec=` line, e.g.:

```ini
Exec=env HANDY_NO_GTK_LAYER_SHELL=1 striper
```

Striper's built-in autostart rewrites its generated desktop entry, so editing that entry is not a durable workaround. For autostart, use an appropriate session/service environment, or disable Striper's built-in autostart and manage your own startup entry with the required variable.

If a workaround helps you, please [open an issue](https://github.com/BenStein1/Striper/issues) describing your distro, desktop environment, and session type — that information helps us narrow down the underlying bug.

### Empty recording overlay on Hyprland / Omarchy

If the recording overlay is empty or bordered, fully quit Striper and launch a
[native installation](BUILD.md#linux-install-from-source) with Wayland enabled:

```bash
env -u HANDY_NO_GTK_LAYER_SHELL GDK_BACKEND=wayland striper
```

If this works, apply `GDK_BACKEND=wayland` only to Striper's launcher. This
workaround does not work with the 0.9.6 AppImage, which forces X11.

### Vulkan Overlays and Capture Tools on Windows (`HANDY_KEEP_VULKAN_IMPLICIT_LAYERS`)

On Windows, Striper asks the Vulkan loader to skip implicit layers to avoid crashes caused by overlay and capture hooks ([Handy #2049](https://github.com/cjpais/Handy/issues/2049)). GPU acceleration remains enabled; this does not change system-wide settings.

To opt out for GPU selection or debugging tools, fully quit Striper (including the tray icon), then run both commands in the same PowerShell window:

```powershell
$env:HANDY_KEEP_VULKAN_IMPLICIT_LAYERS = "1"
& "$env:ProgramFiles\Striper\striper.exe"
```

Adjust the executable path if needed. This override only applies to apps launched from that PowerShell session, not the Start menu. Striper also preserves any existing `VK_LOADER_LAYERS_DISABLE` value.

## Contributing

Striper is developed at [github.com/BenStein1/Striper](https://github.com/BenStein1/Striper). Bug reports and pull requests are welcome there; see [CONTRIBUTING.md](CONTRIBUTING.md) and [BUILD.md](BUILD.md).

If you find a bug that also reproduces in upstream Handy, please consider reporting it to [Handy](https://github.com/cjpais/Handy/issues) as well so everyone benefits.

## Attribution

Striper is a fork of **[Handy](https://github.com/cjpais/Handy)** by **CJ Pais** and the Handy contributors, released under the MIT License. Handy provides the foundation Striper builds on: the Tauri application, local transcription via transcribe.cpp/ggml, Silero VAD, model management, history, global shortcuts, and the clipboard/paste pipeline. Thank you to everyone who built it.

- Upstream repository: <https://github.com/cjpais/Handy>
- Upstream website and documentation: <https://handy.computer>
- Support the upstream project: <https://handy.computer/donate>

As upstream requests, Striper uses its own name, logo, and icons. The Handy name, logo, icon, and brand assets are not open source and are not used by Striper; nothing here implies endorsement by or affiliation with the Handy project. Some identifiers inherited from Handy are kept for compatibility (for example the `HANDY_*` environment variables and the `handy-keys` / `handy-recorder` crates).

## License

MIT License - see [LICENSE](LICENSE) file for details. The original Handy copyright notice is retained as the license requires.

The Striper artwork in [`assets/branding/`](assets/branding/) and the icons derived from it are Striper's own brand assets.

## Acknowledgments

- **Handy** by CJ Pais and contributors, the upstream project
- **Whisper** by OpenAI for the speech recognition model
- **ggml** for an amazing cross-platform tensor library
- **Silero** for great lightweight VAD
- **Tauri** team for the excellent Rust-based app framework
