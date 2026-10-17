//! Keystrokes for verified editing: caret moves, shift-selection, copy,
//! paste, delete and a single undo. Uses the same input tools as dictation
//! paste (enigo, or xdotool/wtype/dotool/ydotool on Linux) so an edit works
//! wherever dictation already does.

use crate::clipboard::with_enigo;
#[cfg(target_os = "linux")]
use crate::clipboard::{
    detect_ydotool_key_syntax, is_dotool_available, is_wtype_available, is_xdotool_available,
    is_ydotool_available, YdotoolKeySyntax,
};
use crate::settings::PasteMethod;
use enigo::{Direction, Key, Keyboard};
#[cfg(target_os = "linux")]
use std::process::Command;
use std::time::Duration;
use tauri::AppHandle;

/// Pause between synthetic key events. Slow targets (Word, Electron apps)
/// drop or reorder events sent back-to-back.
const KEY_GAP: Duration = Duration::from_millis(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKey {
    Left,
    Right,
    ShiftLeft,
    Delete,
    Copy,
    Paste,
    Undo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Enigo,
    #[cfg(target_os = "linux")]
    Xdotool,
    #[cfg(target_os = "linux")]
    Wtype,
    #[cfg(target_os = "linux")]
    Dotool,
    #[cfg(target_os = "linux")]
    Ydotool(YdotoolKeySyntax),
}

pub struct KeySender {
    app: AppHandle,
    backend: Backend,
    paste: PasteMethod,
}

impl KeySender {
    /// Pick the input backend once per edit, mirroring the paste path's
    /// preference order.
    pub fn detect(app: &AppHandle, paste_method: PasteMethod) -> Self {
        let paste = match paste_method {
            PasteMethod::CtrlShiftV | PasteMethod::ShiftInsert => paste_method,
            _ => PasteMethod::CtrlV,
        };
        Self {
            app: app.clone(),
            backend: detect_backend(),
            paste,
        }
    }

    pub fn backend_name(&self) -> String {
        format!("{:?}", self.backend)
    }

    pub fn send(&self, key: EditKey, repeat: usize) -> Result<(), String> {
        if repeat == 0 {
            return Ok(());
        }
        match self.backend {
            Backend::Enigo => with_enigo(&self.app, |enigo| {
                send_enigo(enigo, key, repeat, self.paste)
            }),
            #[cfg(target_os = "linux")]
            Backend::Xdotool => send_xdotool(key, repeat, self.paste),
            #[cfg(target_os = "linux")]
            Backend::Wtype => send_wtype(key, repeat, self.paste),
            #[cfg(target_os = "linux")]
            Backend::Dotool => send_dotool(key, repeat, self.paste),
            #[cfg(target_os = "linux")]
            Backend::Ydotool(syntax) => send_ydotool(key, repeat, self.paste, syntax),
        }
    }
}

#[cfg(target_os = "linux")]
fn detect_backend() -> Backend {
    use crate::utils::{is_gnome_wayland, is_kde_wayland, is_wayland};
    if is_wayland() {
        if !is_kde_wayland() && !is_gnome_wayland() && is_wtype_available() {
            return Backend::Wtype;
        }
        if is_dotool_available() {
            return Backend::Dotool;
        }
        if is_ydotool_available() {
            return Backend::Ydotool(detect_ydotool_key_syntax());
        }
    } else {
        if is_xdotool_available() {
            return Backend::Xdotool;
        }
        if is_ydotool_available() {
            return Backend::Ydotool(detect_ydotool_key_syntax());
        }
    }
    Backend::Enigo
}

#[cfg(not(target_os = "linux"))]
fn detect_backend() -> Backend {
    Backend::Enigo
}

/// Same backend selector used for actual edits. Installation is detected;
/// device permissions/compositor support still require a real field test.
pub fn detected_backend_name() -> &'static str {
    match detect_backend() {
        Backend::Enigo => "enigo",
        #[cfg(target_os = "linux")]
        Backend::Xdotool => "xdotool",
        #[cfg(target_os = "linux")]
        Backend::Wtype => "wtype",
        #[cfg(target_os = "linux")]
        Backend::Dotool => "dotool",
        #[cfg(target_os = "linux")]
        Backend::Ydotool(_) => "ydotool",
    }
}

fn enigo_err(e: enigo::InputError) -> String {
    format!("key injection failed: {e}")
}

#[cfg(target_os = "macos")]
const PRIMARY_MODIFIER: Key = Key::Meta;
#[cfg(not(target_os = "macos"))]
const PRIMARY_MODIFIER: Key = Key::Control;

/// Layout-independent letter keys for shortcuts (virtual key codes on
/// Windows, like the existing paste path).
fn letter(c: char) -> Key {
    #[cfg(target_os = "windows")]
    {
        Key::Other(c.to_ascii_uppercase() as u32)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Key::Unicode(c)
    }
}

fn chord(enigo: &mut enigo::Enigo, modifiers: &[Key], key: Key) -> Result<(), String> {
    for m in modifiers {
        enigo.key(*m, Direction::Press).map_err(enigo_err)?;
    }
    let result = enigo.key(key, Direction::Click).map_err(enigo_err);
    std::thread::sleep(Duration::from_millis(30));
    for m in modifiers.iter().rev() {
        enigo.key(*m, Direction::Release).map_err(enigo_err)?;
    }
    result
}

fn send_enigo(
    enigo: &mut enigo::Enigo,
    key: EditKey,
    repeat: usize,
    paste: PasteMethod,
) -> Result<(), String> {
    match key {
        EditKey::Left | EditKey::Right | EditKey::Delete => {
            let k = match key {
                EditKey::Left => Key::LeftArrow,
                EditKey::Right => Key::RightArrow,
                _ => Key::Delete,
            };
            for _ in 0..repeat {
                enigo.key(k, Direction::Click).map_err(enigo_err)?;
                std::thread::sleep(KEY_GAP);
            }
            Ok(())
        }
        EditKey::ShiftLeft => {
            enigo.key(Key::Shift, Direction::Press).map_err(enigo_err)?;
            let mut result = Ok(());
            for _ in 0..repeat {
                if let Err(e) = enigo.key(Key::LeftArrow, Direction::Click) {
                    result = Err(enigo_err(e));
                    break;
                }
                std::thread::sleep(KEY_GAP);
            }
            enigo
                .key(Key::Shift, Direction::Release)
                .map_err(enigo_err)?;
            result
        }
        EditKey::Copy => chord(enigo, &[PRIMARY_MODIFIER], letter('c')),
        EditKey::Undo => chord(enigo, &[PRIMARY_MODIFIER], letter('z')),
        EditKey::Paste => match paste {
            PasteMethod::CtrlShiftV => crate::input::send_paste_ctrl_shift_v(enigo, 50),
            PasteMethod::ShiftInsert => crate::input::send_paste_shift_insert(enigo, 50),
            _ => crate::input::send_paste_ctrl_v(enigo, 50),
        },
    }
}

#[cfg(target_os = "linux")]
fn run(cmd: &mut Command, tool: &str) -> Result<(), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("Failed to execute {tool}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{tool} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(target_os = "linux")]
fn xdotool_name(key: EditKey, paste: PasteMethod) -> &'static str {
    match key {
        EditKey::Left => "Left",
        EditKey::Right => "Right",
        EditKey::ShiftLeft => "shift+Left",
        EditKey::Delete => "Delete",
        EditKey::Copy => "ctrl+c",
        EditKey::Undo => "ctrl+z",
        EditKey::Paste => match paste {
            PasteMethod::CtrlShiftV => "ctrl+shift+v",
            PasteMethod::ShiftInsert => "shift+Insert",
            _ => "ctrl+v",
        },
    }
}

#[cfg(target_os = "linux")]
fn send_xdotool(key: EditKey, repeat: usize, paste: PasteMethod) -> Result<(), String> {
    run(
        Command::new("xdotool")
            .args(["key", "--clearmodifiers", "--delay", "4", "--repeat"])
            .arg(repeat.to_string())
            .arg(xdotool_name(key, paste)),
        "xdotool",
    )
}

#[cfg(target_os = "linux")]
fn send_wtype(key: EditKey, repeat: usize, paste: PasteMethod) -> Result<(), String> {
    let (mods, k): (&[&str], &str) = match key {
        EditKey::Left => (&[], "Left"),
        EditKey::Right => (&[], "Right"),
        EditKey::ShiftLeft => (&["shift"], "Left"),
        EditKey::Delete => (&[], "Delete"),
        EditKey::Copy => (&["ctrl"], "c"),
        EditKey::Undo => (&["ctrl"], "z"),
        EditKey::Paste => match paste {
            PasteMethod::CtrlShiftV => (&["ctrl", "shift"], "v"),
            PasteMethod::ShiftInsert => (&["shift"], "Insert"),
            _ => (&["ctrl"], "v"),
        },
    };
    let mut args: Vec<&str> = Vec::new();
    for m in mods {
        args.extend(["-M", m]);
    }
    for _ in 0..repeat {
        args.extend(["-k", k]);
    }
    for m in mods.iter().rev() {
        args.extend(["-m", m]);
    }
    run(Command::new("wtype").args(&args), "wtype")
}

#[cfg(target_os = "linux")]
fn send_dotool(key: EditKey, repeat: usize, paste: PasteMethod) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;
    let name = xdotool_name(key, paste).to_lowercase();
    let mut child = Command::new("dotool")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn dotool: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        writeln!(stdin, "keydelay 4").map_err(|e| e.to_string())?;
        for _ in 0..repeat {
            writeln!(stdin, "key {name}").map_err(|e| e.to_string())?;
        }
    }
    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait for dotool: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("dotool failed".into())
    }
}

#[cfg(target_os = "linux")]
fn send_ydotool(
    key: EditKey,
    repeat: usize,
    paste: PasteMethod,
    syntax: YdotoolKeySyntax,
) -> Result<(), String> {
    let mut args: Vec<String> = vec!["key".into()];
    match syntax {
        YdotoolKeySyntax::Symbolic => {
            for _ in 0..repeat {
                args.push(xdotool_name(key, paste).into());
            }
        }
        YdotoolKeySyntax::RawKeycodes => {
            // Linux input event codes.
            const CTRL: u16 = 29;
            const SHIFT: u16 = 42;
            let (mods, k): (&[u16], u16) = match key {
                EditKey::Left => (&[], 105),
                EditKey::Right => (&[], 106),
                EditKey::ShiftLeft => (&[SHIFT], 105),
                EditKey::Delete => (&[], 111),
                EditKey::Copy => (&[CTRL], 46),
                EditKey::Undo => (&[CTRL], 44),
                EditKey::Paste => match paste {
                    PasteMethod::CtrlShiftV => (&[CTRL, SHIFT], 47),
                    PasteMethod::ShiftInsert => (&[SHIFT], 110),
                    _ => (&[CTRL], 47),
                },
            };
            args.extend(mods.iter().map(|m| format!("{m}:1")));
            for _ in 0..repeat {
                args.push(format!("{k}:1"));
                args.push(format!("{k}:0"));
            }
            args.extend(mods.iter().rev().map(|m| format!("{m}:0")));
        }
    }
    run(Command::new("ydotool").args(&args), "ydotool")
}
