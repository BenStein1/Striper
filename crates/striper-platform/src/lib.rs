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

/// Check an executable on PATH without spawning a shell or `which` process.
/// This detects installation only, not compositor/device permissions.
pub fn command_available(name: &str) -> bool {
    command_available_on_path(name, std::env::var_os("PATH").as_deref())
}

fn command_available_on_path(name: &str, path: Option<&std::ffi::OsStr>) -> bool {
    if name.is_empty() || name.contains(['/', '\\']) {
        return false;
    }
    let Some(path) = path else {
        return false;
    };
    std::env::split_paths(path).any(|directory| {
        let candidate = directory.join(name);
        executable_file(&candidate)
            || (cfg!(windows) && executable_file(&candidate.with_extension("exe")))
    })
}
fn executable_file(path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Best-effort identity of the currently focused window.
pub fn foreground_window() -> ForegroundWindow {
    imp::foreground_window()
}

/// Minimal foreground identity for hot-path focus checks. On KDE Wayland
/// this performs one bounded kdotool query, avoiding PID/title subprocesses.
pub fn foreground_window_token() -> ForegroundWindow {
    imp::foreground_window_token()
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

    pub fn foreground_window_token() -> ForegroundWindow {
        foreground_window()
    }

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

    pub fn foreground_window_token() -> ForegroundWindow {
        if is_wayland() {
            return ForegroundWindow {
                window_id: run(&["getactivewindow"]),
                source: "kdotool",
                ..Default::default()
            };
        }
        foreground_window()
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
        bounded_output(
            Command::new("kdotool").args(args),
            std::time::Duration::from_millis(250),
        )
    }

    /// A stalled compositor/DBus connection must not hang the audio consumer
    /// or leave an editing worker waiting forever. kdotool output is tiny;
    /// unread pipe backpressure is also bounded by this deadline.
    fn bounded_output(command: &mut Command, timeout: std::time::Duration) -> Option<String> {
        use std::process::Stdio;
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return None;
                    }
                    let out = child.wait_with_output().ok()?;
                    return String::from_utf8(out.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                }
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
    }

    #[cfg(test)]
    mod probe_tests {
        use super::*;
        use std::time::{Duration, Instant};
        #[test]
        fn stalled_focus_helper_is_killed_and_reaped() {
            let started = Instant::now();
            assert!(bounded_output(
                Command::new("sh").args(["-c", "exec sleep 5"]),
                Duration::from_millis(30)
            )
            .is_none());
            assert!(started.elapsed() < Duration::from_secs(2));
        }
        #[test]
        fn focus_helper_output_is_trimmed_and_failures_are_unknown() {
            assert_eq!(
                bounded_output(
                    Command::new("printf").arg("  {window-id}\n"),
                    Duration::from_secs(1)
                )
                .as_deref(),
                Some("{window-id}")
            );
            assert!(bounded_output(
                Command::new("sh").args(["-c", "exit 1"]),
                Duration::from_secs(1)
            )
            .is_none());
        }
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

    pub fn foreground_window_token() -> ForegroundWindow {
        foreground_window()
    }
    pub fn modifiers_held() -> Option<bool> {
        None
    }

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
    fn tool_detection_does_not_accept_directories_or_missing_path() {
        let dir = std::env::temp_dir();
        assert!(!command_available_on_path("anything", None));
        assert!(!command_available_on_path(
            "../anything",
            Some(dir.as_os_str())
        ));
        assert!(!command_available_on_path("", Some(dir.as_os_str())));
        assert!(!executable_file(&dir));
    }

    #[cfg(unix)]
    #[test]
    fn tools_must_be_executable_and_are_found_without_processes() {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::env::temp_dir().join(format!("striper-tool-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("striper-probe-tool");
        std::fs::write(&file, "test").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!command_available_on_path(
            "striper-probe-tool",
            Some(directory.as_os_str())
        ));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(command_available_on_path(
            "striper-probe-tool",
            Some(directory.as_os_str())
        ));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn probing_never_panics() {
        // In CI there is usually no display; the probe must degrade quietly.
        let w = foreground_window();
        assert!(!w.source.is_empty());
    }
}
