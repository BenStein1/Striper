//! The real-application [`TextTarget`]: drives the focused text field with
//! keystrokes and uses the clipboard for selection readback and
//! replacement, preserving the user's clipboard around the whole edit.

use super::keys::{EditKey, KeySender};
use crate::clipboard::write_text_to_clipboard;
use crate::settings::{AppSettings, PasteMethod};
use log::debug;
use std::time::{Duration, Instant};
use striper_core::edit::TextTarget;
use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;

/// How long to wait for the target app to answer a copy request.
const COPY_TIMEOUT: Duration = Duration::from_millis(600);
const COPY_POLL: Duration = Duration::from_millis(15);

fn read_clipboard_text(app: &AppHandle) -> Option<String> {
    #[cfg(target_os = "linux")]
    if crate::utils::is_wayland() {
        if let Ok(out) = std::process::Command::new("wl-paste")
            .args(["--no-newline"])
            .output()
        {
            if out.status.success() {
                return Some(String::from_utf8_lossy(&out.stdout).into_owned());
            }
        }
    }
    app.clipboard().read_text().ok()
}

/// Saves the clipboard on creation and puts it back on [`ClipboardGuard::restore`].
pub struct ClipboardGuard {
    app: AppHandle,
    text: Option<String>,
    image: Option<tauri::image::Image<'static>>,
}

impl ClipboardGuard {
    pub fn save(app: &AppHandle) -> Self {
        let clipboard = app.clipboard();
        let text = clipboard.read_text().ok().filter(|t| !t.is_empty());
        let image = if text.is_none() {
            clipboard.read_image().ok().map(|i| i.to_owned())
        } else {
            None
        };
        Self {
            app: app.clone(),
            text,
            image,
        }
    }

    pub fn restore(self, settle: Duration) {
        // Give the target time to finish reading our last paste.
        std::thread::sleep(settle);
        let clipboard = self.app.clipboard();
        if let Some(text) = self.text {
            let _ = write_text_to_clipboard(&self.app, &text);
        } else if let Some(image) = self.image {
            let _ = clipboard.write_image(&image);
        } else {
            let _ = clipboard.clear();
        }
    }
}

pub struct KeyboardTarget {
    app: AppHandle,
    keys: KeySender,
    paste_delay: Duration,
    paste_settle: Duration,
    expected_target: striper_core::session::AppIdentity,
}

impl KeyboardTarget {
    pub fn new(
        app: &AppHandle,
        settings: &AppSettings,
        expected_target: striper_core::session::AppIdentity,
    ) -> Self {
        let keys = KeySender::detect(app, settings.paste_method);
        debug!(
            "Correction edit using input backend {}",
            keys.backend_name()
        );
        Self {
            app: app.clone(),
            expected_target,
            keys,
            paste_delay: Duration::from_millis(settings.paste_delay_ms.max(30)),
            paste_settle: Duration::from_millis(settings.paste_delay_after_ms.max(120)),
        }
    }

    fn check_focus(&self) -> Result<(), String> {
        if !self
            .expected_target
            .permits_continuation(&super::focused_window_token())
        {
            return Err(
                "The original window is no longer focused or observable; stopped editing.".into(),
            );
        }
        Ok(())
    }

    pub fn settle(&self) -> Duration {
        self.paste_settle
    }
}

impl TextTarget for KeyboardTarget {
    fn move_left(&mut self, steps: usize) -> Result<(), String> {
        self.check_focus()?;
        self.keys.send(EditKey::Left, steps)
    }

    fn move_right(&mut self, steps: usize) -> Result<(), String> {
        self.check_focus()?;
        self.keys.send(EditKey::Right, steps)
    }

    fn select_left(&mut self, steps: usize) -> Result<(), String> {
        self.check_focus()?;
        self.keys.send(EditKey::ShiftLeft, steps)
    }

    fn read_selection(&mut self) -> Result<Option<String>, String> {
        self.check_focus()?;
        // A unique sentinel tells "the app copied" apart from "nothing
        // happened" (no selection support, copy blocked).
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let sentinel = format!("\u{2063}striper-readback-{nanos}");
        write_text_to_clipboard(&self.app, &sentinel)?;
        std::thread::sleep(self.paste_delay);
        self.keys.send(EditKey::Copy, 1)?;
        let started = Instant::now();
        while started.elapsed() < COPY_TIMEOUT {
            std::thread::sleep(COPY_POLL);
            if let Some(text) = read_clipboard_text(&self.app) {
                if text != sentinel {
                    return Ok(Some(text));
                }
            }
        }
        debug!("Selection readback timed out; the target did not copy");
        Ok(None)
    }

    fn replace_selection(&mut self, text: &str) -> Result<(), String> {
        self.check_focus()?;
        let settings = crate::settings::get_settings(&self.app);
        if settings.paste_method == PasteMethod::Direct {
            // Typing replaces the selection without touching the clipboard.
            crate::clipboard::paste_direct(
                text,
                &self.app,
                #[cfg(target_os = "linux")]
                settings.typing_tool,
            )?;
        } else {
            write_text_to_clipboard(&self.app, text)?;
            std::thread::sleep(self.paste_delay);
            self.keys.send(EditKey::Paste, 1)?;
        }
        std::thread::sleep(self.paste_settle);
        Ok(())
    }

    fn delete_selection(&mut self) -> Result<(), String> {
        self.check_focus()?;
        self.keys.send(EditKey::Delete, 1)
    }

    fn collapse_right(&mut self) -> Result<(), String> {
        self.check_focus()?;
        self.keys.send(EditKey::Right, 1)
    }

    fn undo_once(&mut self) -> Result<(), String> {
        self.check_focus()?;
        self.keys.send(EditKey::Undo, 1)?;
        std::thread::sleep(self.paste_settle);
        Ok(())
    }
}
