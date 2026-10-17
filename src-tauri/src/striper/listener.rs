//! Opt-in, local CPU listener. Idle audio is never persisted or sent to an
//! endpoint. Vosk only detects the wake prefix and utterance boundaries;
//! correction audio uses the already-selected Handy recognizer.
use super::{focused_identity, now_ms, state};
use crate::audio_toolkit::audio::AudioFrameCallback;
use crate::managers::{audio::AudioRecordingManager, transcription::TranscriptionManager};
use crate::settings::{get_settings, write_settings};
use crate::TranscriptionCoordinator;
use serde::Serialize;
use specta::Type;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use striper_core::session::{AppIdentity, TargetMatch};
use striper_listener::{wake_prefix, Conversation, Detector};
use tauri::{AppHandle, Emitter, Manager};

const MAX_AUDIO: usize = 20 * 16_000;

#[derive(Clone, Default, Serialize, Type)]
pub struct ListenerStatus {
    pub installed: bool,
    pub enabled: bool,
    pub phase: String,
    pub error: Option<String>,
}
#[derive(Default)]
pub struct ListenerState(pub Mutex<ListenerStatus>);

fn report(app: &AppHandle, phase: &str, error: Option<String>) {
    if let Some(s) = app.try_state::<ListenerState>() {
        if let Ok(mut status) = s.0.lock() {
            status.phase = phase.into();
            status.error = error;
            status.enabled = get_settings(app).correction_listener_enabled;
            status.installed =
                assets(app).is_ok_and(|(lib, model)| lib.is_file() && model.is_dir());
            let _ = app.emit("correction-listener-status", status.clone());
        }
    }
}

pub fn assets(app: &AppHandle) -> Result<(PathBuf, PathBuf), String> {
    let base = crate::portable::app_data_dir(app)
        .map_err(|e| e.to_string())?
        .join("listener");
    let library = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        base.join("vosk-win64-0.3.45/libvosk.dll")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        base.join("vosk-linux-x86_64-0.3.45/libvosk.so")
    } else {
        return Err("The wake listener currently supports Windows and Linux x64.".into());
    };
    Ok((library, base.join("vosk-model-small-en-us-0.15")))
}

pub fn init(app: &AppHandle) {
    app.manage(ListenerState::default());
    let app = app.clone();
    std::thread::spawn(move || loop {
        if let Err(error) = run(&app) {
            log::error!("Wake listener stopped: {error}");
            if let Some(rm) = app.try_state::<Arc<AudioRecordingManager>>() {
                rm.stop_listener_capture();
            }
            // Preserve the failure visibly; retry only after an explicit toggle.
            report(&app, "error", Some(error));
            while get_settings(&app).correction_listener_enabled {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    });
}

/// Only wake against a known, recent dictation in the focused window. Unknown
/// Wayland focus uses the explicit shortcut rather than ambient activation.
fn target(app: &AppHandle) -> Option<AppIdentity> {
    let focused = focused_identity();
    let s = state(app)?;
    let ledger = s.ledger.lock().ok()?;
    // App-name equality cannot distinguish two documents/windows. Ambient
    // activation needs an actual window identity, unlike the hotkey fallback.
    let original_window = ledger.target()?.window_id.as_deref()?;
    if focused.window_id.as_deref()? != original_window {
        return None;
    }
    if ledger.target()?.compare(&focused) != TargetMatch::Same
        || ledger.check_target(&focused, now_ms()).is_err()
    {
        return None;
    }
    Some(focused)
}

fn run(app: &AppHandle) -> Result<(), String> {
    let rm = app.state::<Arc<AudioRecordingManager>>().inner().clone();
    let (tx, rx) = mpsc::sync_channel::<(u64, Vec<f32>)>(32);
    let overflow = Arc::new(AtomicBool::new(false));
    let mut detector: Option<Detector> = None;
    let mut conversation = Conversation::default();
    let mut session_target: Option<AppIdentity> = None;
    let mut audio = Vec::new();
    let mut generation = u64::MAX;
    let mut utterance_woke = false;
    let mut phase = String::new();
    let clock = Instant::now();
    let mut last_frame = Instant::now();
    let mut cancel_generation = rm.cancel_generation();
    loop {
        let enabled = get_settings(app).correction_listener_enabled;
        if rm.cancel_generation() != cancel_generation {
            cancel_generation = rm.cancel_generation();
            rm.stop_listener_capture();
            audio.clear();
            if let Some(d) = detector.as_mut() {
                d.reset();
            }
            conversation.reset();
            session_target = None;
            utterance_woke = false;
            if let Some(s) = app.try_state::<ListenerResume>() {
                if let Ok(mut resume) = s.0.lock() {
                    *resume = None;
                }
            }
            while rx.try_recv().is_ok() {}
        }
        let idle = app
            .try_state::<TranscriptionCoordinator>()
            .is_some_and(|c| c.is_idle());
        if !enabled || !idle {
            rm.stop_listener_capture();
            audio.clear();
            if let Some(d) = detector.as_mut() {
                d.reset();
            }
            if !enabled {
                detector = None;
                if let Some(s) = app.try_state::<ListenerResume>() {
                    if let Ok(mut resume) = s.0.lock() {
                        *resume = None;
                    }
                }
            }
            conversation.reset();
            session_target = None;
            utterance_woke = false;
            while rx.try_recv().is_ok() {}
            let next = if enabled { "paused" } else { "off" };
            if phase != next {
                report(app, next, None);
                phase = next.into();
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        if detector.is_none() {
            report(app, "loading", None);
            let loaded = assets(app).and_then(|(lib, model)| Detector::open(&lib, &model));
            match loaded {
                Ok(d) => detector = Some(d),
                Err(error) => {
                    report(app, "error", Some(error));
                    // Remain alive so toggling off/on can retry after setup.
                    while get_settings(app).correction_listener_enabled {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    continue;
                }
            }
        }
        let epoch = rm.listener_epoch();
        let tx = tx.clone();
        let overrun = Arc::clone(&overflow);
        let callback: AudioFrameCallback = Arc::new(move |frame| {
            // No blocking/native inference on the recorder delivery thread.
            if tx
                .try_send((epoch.load(Ordering::Acquire), frame.to_vec()))
                .is_err()
            {
                overrun.store(true, Ordering::Release);
            }
        });
        match rm.start_listener_capture(callback) {
            Ok(g) if g != generation => {
                generation = g;
                while rx.try_recv().is_ok() {}
                audio.clear();
                detector.as_mut().expect("loaded above").reset();
                conversation.reset();
                session_target = None;
                utterance_woke = false;
                last_frame = Instant::now();
            }
            Ok(_) => {}
            Err(error) => {
                if !rm.is_recording() {
                    report(app, "error", Some(error));
                }
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
        }
        if overflow.swap(false, Ordering::AcqRel) {
            audio.clear();
            detector.as_mut().expect("loaded above").reset();
            conversation.reset();
            session_target = None;
            utterance_woke = false;
            while rx.try_recv().is_ok() {}
        }
        let now = clock.elapsed().as_millis() as u64;
        let current = target(app);
        if session_target.as_ref().is_some_and(|t| {
            current
                .as_ref()
                .is_none_or(|c| t.compare(c) != TargetMatch::Same)
        }) {
            conversation.reset();
            session_target = None;
            utterance_woke = false;
            audio.clear();
            detector.as_mut().expect("loaded above").reset();
        }
        let next = if conversation.active(now) || utterance_woke {
            "listening"
        } else {
            "armed"
        };
        if phase != next {
            report(app, next, None);
            phase = next.into();
        }
        let (epoch, frame) = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => frame,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_frame.elapsed() > Duration::from_secs(5) {
                    rm.stop_listener_capture();
                    return Err("The microphone stopped delivering audio. Check the input device and restart Striper.".into());
                }
                continue;
            }
            Err(_) => return Err("Microphone audio channel closed".into()),
        };
        if rm.cancel_generation() != cancel_generation
            || epoch != generation
            || rm.listener_generation() != generation
        {
            continue;
        }
        last_frame = Instant::now();
        if audio.len() + frame.len() > MAX_AUDIO {
            // Bound memory and reject truncated/overlong utterances.
            audio.clear();
            detector.as_mut().expect("loaded above").reset();
            conversation.reset();
            session_target = None;
            utterance_woke = false;
            continue;
        }
        audio.extend_from_slice(&frame);
        let detection = detector.as_mut().expect("loaded above").feed(&frame)?;
        // Preserve two seconds of pre-roll while idle, rather than letting
        // silence consume the bounded instruction buffer.
        if detection.text.is_empty() && audio.len() > 32_000 {
            audio.drain(..audio.len() - 32_000);
        }
        if current.is_some() && wake_prefix(&detection.text).is_some() && !utterance_woke {
            session_target = current.clone();
            utterance_woke = true;
            // No chime here: speaker feedback could contaminate same-breath
            // instructions. The tray/overlay provides a visual acknowledgment.
            crate::utils::show_feedback_overlay(app, "Checking correction wake phrase…", "info");
        }
        if detection.endpoint {
            let prefix = wake_prefix(&detection.text);
            // CPU output is only a candidate. The selected recognizer must
            // confirm an initial wake, including a standalone wake phrase.
            let require_wake = !conversation.active(now);
            let candidate =
                prefix.is_some() || (!require_wake && !detection.text.trim().is_empty());
            if candidate && current.is_some() {
                let samples = std::mem::take(&mut audio);
                if let Some(c) = app.try_state::<TranscriptionCoordinator>() {
                    c.voice_correction(
                        samples,
                        current.clone().unwrap_or_default(),
                        cancel_generation,
                        require_wake,
                    );
                }
                conversation.reset();
                session_target = None;
            } else {
                audio.clear();
            }
            utterance_woke = false;
        }
        // A completed correction may reopen the conversational window. Never
        // reopen across a focus switch or after a user cancellation.
        if let Some(s) = app.try_state::<ListenerResume>() {
            if let Ok(mut resume) = s.0.lock() {
                if let Some(t) = resume.take() {
                    if current
                        .as_ref()
                        .is_some_and(|c| t.compare(c) == TargetMatch::Same)
                    {
                        conversation.wake(now);
                        session_target = Some(t);
                    }
                }
            }
        }
    }
}

#[derive(Default)]
pub struct ListenerResume(pub Mutex<Option<AppIdentity>>);

/// Called only after the coordinator reserves its Processing stage.
pub fn transcribe_instruction(
    app: AppHandle,
    samples: Vec<f32>,
    original: AppIdentity,
    generation: u64,
    require_wake: bool,
) {
    tauri::async_runtime::spawn(async move {
        struct Finished(AppHandle);
        impl Drop for Finished {
            fn drop(&mut self) {
                crate::shortcut::unregister_cancel_shortcut(&self.0);
                if let Some(tm) = self.0.try_state::<Arc<TranscriptionManager>>() {
                    tm.maybe_unload_immediately("voice correction");
                }
                if let Some(c) = self.0.try_state::<TranscriptionCoordinator>() {
                    c.notify_processing_finished();
                }
            }
        }
        let _finished = Finished(app.clone());
        crate::shortcut::register_cancel_shortcut(&app);
        report(&app, "processing", None);
        let tm = app.state::<Arc<TranscriptionManager>>().inner().clone();
        let result =
            tauri::async_runtime::spawn_blocking(move || tm.transcribe_recording(samples)).await;
        let rm = app.state::<Arc<AudioRecordingManager>>();
        if rm.was_cancelled_since(generation)
            || original.compare(&focused_identity()) != TargetMatch::Same
        {
            return;
        }
        match result {
            Ok(Ok(text)) => {
                if require_wake {
                    let Some(rest) = wake_prefix(&text) else {
                        // The narrow CPU grammar can confuse similar speech.
                        // This independent ASR confirmation refuses it before
                        // parser/LLM/clipboard access or session activation.
                        log::debug!("Wake candidate rejected by selected recognizer");
                        return;
                    };
                    if rest.is_empty() {
                        if !get_settings(&app).correction_listener_enabled {
                            return;
                        }
                        if let Some(s) = app.try_state::<ListenerResume>() {
                            if let Ok(mut resume) = s.0.lock() {
                                *resume = Some(original);
                            }
                        }
                        crate::utils::show_feedback_overlay(
                            &app,
                            "Listening for a correction…",
                            "info",
                        );
                        return;
                    }
                }
                super::correction::handle_instruction_guarded(app.clone(), text, generation, true)
                    .await;
                if rm.was_cancelled_since(generation) {
                    return;
                }
                let open = state(&app)
                    .and_then(|s| s.session.lock().ok().map(|session| session.is_some()))
                    .unwrap_or(false);
                if open {
                    if let Some(s) = app.try_state::<ListenerResume>() {
                        if let Ok(mut resume) = s.0.lock() {
                            *resume = Some(original);
                        }
                    }
                }
            }
            Ok(Err(e)) => report(
                &app,
                "error",
                Some(format!("Correction transcription failed: {e}")),
            ),
            Err(e) => report(
                &app,
                "error",
                Some(format!("Correction worker failed: {e}")),
            ),
        }
    });
}

#[tauri::command]
#[specta::specta]
pub fn get_correction_listener_status(app: AppHandle) -> Result<ListenerStatus, String> {
    let s = app.state::<ListenerState>();
    let mut status = s.0.lock().map_err(|e| e.to_string())?.clone();
    status.enabled = get_settings(&app).correction_listener_enabled;
    status.installed = assets(&app).is_ok_and(|(lib, model)| lib.is_file() && model.is_dir());
    Ok(status)
}

#[tauri::command]
#[specta::specta]
pub fn change_correction_listener_enabled_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    if enabled {
        let (lib, model) = assets(&app)?;
        if !lib.is_file() || !model.is_dir() {
            return Err("Install the local listener files first.".into());
        }
    }
    let mut s = get_settings(&app);
    s.correction_listener_enabled = enabled;
    write_settings(&app, s);
    Ok(())
}
