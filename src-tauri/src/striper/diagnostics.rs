//! Read-only desktop compatibility inspection. Never opens the microphone,
//! reads clipboard contents, injects keys, or contacts a provider.
use serde::Serialize;
use specta::Type;
use tauri::AppHandle;

#[derive(Serialize, Type)]
pub struct ToolAvailability {
    pub name: String,
    pub installed: bool,
}
#[derive(Serialize, Type)]
pub struct CorrectionCapabilities {
    pub os: String,
    pub session: String,
    pub desktop: String,
    pub focus_source: String,
    pub window_observable: bool,
    pub input_backend: String,
    pub readback_required: bool,
    pub listener_supported: bool,
    pub tools: Vec<ToolAvailability>,
    /// Stable translation keys; never expose document titles/window handles.
    pub guidance: Vec<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn get_correction_capabilities(app: AppHandle) -> Result<CorrectionCapabilities, String> {
    tauri::async_runtime::spawn_blocking(move || inspect(&app))
        .await
        .map_err(|e| e.to_string())
}

fn inspect(app: &AppHandle) -> CorrectionCapabilities {
    let settings = crate::settings::get_settings(app);
    let foreground = striper_platform::foreground_window_token();
    let input_backend = super::keys::detected_backend_name();
    let linux = cfg!(target_os = "linux");
    #[cfg(target_os = "linux")]
    let (wayland, kde_wayland) = (crate::utils::is_wayland(), crate::utils::is_kde_wayland());
    #[cfg(not(target_os = "linux"))]
    let (wayland, kde_wayland) = (false, false);
    let session = if wayland {
        "wayland"
    } else if linux && std::env::var_os("DISPLAY").is_some() {
        "x11"
    } else if linux {
        "headless"
    } else {
        "native"
    };
    let tools = if linux {
        [
            "kdotool", "xdotool", "dotool", "ydotool", "wtype", "wl-copy", "wl-paste",
        ]
        .into_iter()
        .map(|name| ToolAvailability {
            name: name.into(),
            installed: striper_platform::command_available(name),
        })
        .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let installed = |name: &str| tools.iter().any(|tool| tool.name == name && tool.installed);
    let mut guidance = Vec::new();
    if foreground.window_id.is_none() {
        guidance.push("focusUnknown".into());
    }
    if wayland && kde_wayland && !installed("kdotool") {
        guidance.push("kdeFocusHelper".into());
    }
    if wayland && input_backend == "enigo" {
        guidance.push("waylandInputHelper".into());
    }
    if wayland && (!installed("wl-copy") || !installed("wl-paste")) {
        guidance.push("waylandClipboard".into());
    }
    if !settings.correction_require_readback {
        guidance.push("readbackDisabled".into());
    }
    if input_backend == "dotool" || input_backend == "ydotool" {
        guidance.push("inputPermissions".into());
    }
    guidance.push("realFieldTest".into());
    CorrectionCapabilities {
        os: std::env::consts::OS.into(),
        session: session.into(),
        desktop: if linux {
            std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default()
        } else {
            String::new()
        },
        focus_source: foreground.source.into(),
        window_observable: foreground.window_id.is_some(),
        input_backend: input_backend.into(),
        readback_required: settings.correction_require_readback,
        listener_supported: cfg!(all(
            target_arch = "x86_64",
            any(target_os = "windows", target_os = "linux")
        )),
        tools,
        guidance,
    }
}
