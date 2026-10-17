//! Platform adapters (PLAN.md §8 `platform-windows` / `platform-linux`).
//!
//! Currently: identity of the focused window, which the correction lane
//! compares against the window a dictation was inserted into before it
//! edits anything. Every probe is best-effort and returns `None` fields when
//! the desktop does not expose the information (e.g. most Wayland
//! compositors), never an error.

/// Identity of the focused top-level window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForegroundWindow {
    /// Lowercased executable name ("notepad.exe", "kate").
    pub app_id: Option<String>,
    /// Opaque window handle (HWND, X11 window id, KWin UUID).
    pub window_id: Option<String>,
    pub title: Option<String>,
    /// How the identity was obtained, for diagnostics.
    pub source: &'static str,
}

/// Best-effort identity of the currently focused window.
pub fn foreground_window() -> ForegroundWindow {
    imp::foreground_window()
}

/// Whether Ctrl, Alt, Shift or the Windows/Super key is physically held.
/// Synthetic arrow keys sent while the user still holds a modifier (e.g. the
/// tail end of a push-to-talk chord) would jump words or extend selections,
/// so edits wait for this to clear. `None` when it cannot be determined.
pub fn modifiers_held() -> Option<bool> {
    imp::modifiers_held()
}

/// Lowercased file name of an executable path.
pub fn exe_name(path: &str) -> Option<String> {
    let name = path.rsplit(['/', '\\']).next()?.trim();
    (!name.is_empty()).then(|| name.to_lowercase())
}

#[cfg(windows)]
mod imp {
    use super::{exe_name, ForegroundWindow};
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };

    pub fn modifiers_held() -> Option<bool> {
        // SAFETY: GetAsyncKeyState only reads global key state.
        let down = |vk: u16| unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 };
        Some(
            [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
                .iter()
                .any(|k| down(k.0)),
        )
    }

    pub fn foreground_window() -> ForegroundWindow {
        // SAFETY: plain Win32 queries on handles we do not own; every handle
        // we open is closed before returning.
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0.is_null() {
                return ForegroundWindow {
                    source: "win32",
                    ..Default::default()
                };
            }
            let mut title_buf = [0u16; 512];
            let len = GetWindowTextW(hwnd, &mut title_buf);
            let title = (len > 0).then(|| String::from_utf16_lossy(&title_buf[..len as usize]));

            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let mut app_id = None;
            if pid != 0 {
                if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                    let mut buf = [0u16; 1024];
                    let mut size = buf.len() as u32;
                    if QueryFullProcessImageNameW(
                        process,
                        PROCESS_NAME_WIN32,
                        PWSTR(buf.as_mut_ptr()),
                        &mut size,
                    )
                    .is_ok()
                    {
                        app_id = exe_name(&String::from_utf16_lossy(&buf[..size as usize]));
                    }
                    let _ = CloseHandle(process);
                }
            }
            ForegroundWindow {
                app_id,
                window_id: Some(format!("{:x}", hwnd.0 as usize)),
                title,
                source: "win32",
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{exe_name, ForegroundWindow};
    use std::process::Command;

    fn is_wayland() -> bool {
        std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
    }

    fn app_id_for_pid(pid: u32) -> Option<String> {
        std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .and_then(|p| exe_name(&p.to_string_lossy()))
            .or_else(|| {
                std::fs::read_to_string(format!("/proc/{pid}/comm"))
                    .ok()
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
            })
    }

    pub fn modifiers_held() -> Option<bool> {
        if is_wayland() {
            return None;
        }
        x11::modifiers_held()
    }

    pub fn foreground_window() -> ForegroundWindow {
        if is_wayland() {
            // On Wayland the X11 view only covers XWayland clients, so it would
            // misreport focus. KDE users can install kdotool, which asks KWin.
            return kdotool().unwrap_or(ForegroundWindow {
                source: "wayland-unknown",
                ..Default::default()
            });
        }
        x11::foreground_window().unwrap_or(ForegroundWindow {
            source: "x11-unavailable",
            ..Default::default()
        })
    }

    fn run(args: &[&str]) -> Option<String> {
        let out = Command::new("kdotool").args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn kdotool() -> Option<ForegroundWindow> {
        let id = run(&["getactivewindow"])?;
        let pid = run(&["getwindowpid", &id]).and_then(|p| p.parse::<u32>().ok());
        Some(ForegroundWindow {
            app_id: pid.and_then(app_id_for_pid),
            title: run(&["getwindowname", &id]),
            window_id: Some(id),
            source: "kdotool",
        })
    }

    mod x11 {
        use super::{app_id_for_pid, ForegroundWindow};
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, Window};
        use x11rb::rust_connection::RustConnection;

        pub fn modifiers_held() -> Option<bool> {
            use x11rb::protocol::xproto::KeyButMask;
            let (conn, screen) = x11rb::connect(None).ok()?;
            let root = conn.setup().roots.get(screen)?.root;
            let mask = conn.query_pointer(root).ok()?.reply().ok()?.mask;
            let held =
                KeyButMask::SHIFT | KeyButMask::CONTROL | KeyButMask::MOD1 | KeyButMask::MOD4;
            Some(u16::from(mask) & u16::from(held) != 0)
        }

        fn atom(conn: &RustConnection, name: &str) -> Option<u32> {
            Some(
                conn.intern_atom(false, name.as_bytes())
                    .ok()?
                    .reply()
                    .ok()?
                    .atom,
            )
        }

        pub fn foreground_window() -> Option<ForegroundWindow> {
            let (conn, screen) = x11rb::connect(None).ok()?;
            let root = conn.setup().roots.get(screen)?.root;
            let active = atom(&conn, "_NET_ACTIVE_WINDOW")?;
            let reply = conn
                .get_property(false, root, active, AtomEnum::WINDOW, 0, 1)
                .ok()?
                .reply()
                .ok()?;
            let window: Window = reply.value32()?.next()?;
            if window == 0 {
                return None;
            }
            let pid_atom = atom(&conn, "_NET_WM_PID")?;
            let pid = conn
                .get_property(false, window, pid_atom, AtomEnum::CARDINAL, 0, 1)
                .ok()
                .and_then(|c| c.reply().ok())
                .and_then(|r| r.value32().and_then(|mut v| v.next()));
            let utf8 = atom(&conn, "UTF8_STRING");
            let name_atom = atom(&conn, "_NET_WM_NAME");
            let title = match (name_atom, utf8) {
                (Some(n), Some(u)) => conn
                    .get_property(false, window, n, u, 0, 1024)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.value).into_owned())
                    .filter(|s| !s.is_empty()),
                _ => None,
            };
            Some(ForegroundWindow {
                app_id: pid.and_then(app_id_for_pid),
                window_id: Some(format!("{window:#x}")),
                title,
                source: "x11",
            })
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::ForegroundWindow;

    pub fn foreground_window() -> ForegroundWindow {
        ForegroundWindow {
            source: "unsupported",
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_names_are_lowercased_basenames() {
        assert_eq!(
            exe_name(r"C:\Windows\System32\NOTEPAD.EXE").as_deref(),
            Some("notepad.exe")
        );
        assert_eq!(exe_name("/usr/bin/kate").as_deref(), Some("kate"));
        assert_eq!(exe_name(""), None);
    }

    #[test]
    fn probing_never_panics() {
        // In CI there is usually no display; the probe must degrade quietly.
        let w = foreground_window();
        assert!(!w.source.is_empty());
    }
}
