//! Hands-free dictation session (PLAN.md §5a): Ctrl+Space or the big button
//! starts it, speech is committed at natural pauses, "mute"/"unmute" pause
//! and resume by voice, and the same control ends it.
//!
//! One thread segments idle microphone audio into utterances with the
//! selected VAD; a second transcribes them in order with the selected
//! recognizer and acts on what `striper_core::dictation` decides. While
//! paused, long utterances are dropped untranscribed and short ones are
//! checked locally for a standalone "unmute"; nothing said while paused is
//! inserted, stored or sent anywhere. Off releases the microphone.
use super::focused_identity;
use crate::audio_feedback::{play_feedback_sound, SoundType};
use crate::audio_toolkit::audio::AudioFrameCallback;
use crate::managers::{audio::AudioRecordingManager, transcription::TranscriptionManager};
use crate::settings::get_settings;
use crate::tray::{set_tray_state, TrayIconState};
use crate::TranscriptionCoordinator;
use log::{debug, error, info, warn};
use serde::Serialize;
use specta::Type;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use striper_core::dictation::{HandsFreeSession, SessionState, UtteranceAction, UtteranceTiming};
use striper_core::session::{AppIdentity, TargetMatch};
use tauri::{AppHandle, Emitter, Manager};

const SAMPLE_RATE: usize = 16_000;
/// Silence that ends an utterance (a natural pause).
const END_SILENCE_MS: u64 = 700;
/// Voiced audio needed before an utterance starts (rejects clicks).
const ONSET_MS: u64 = 90;
/// Audio kept from before the onset so first syllables aren't clipped.
const PRE_ROLL_MS: u64 = 300;
/// Long monologues are cut so text keeps appearing.
const MAX_UTTERANCE_MS: u64 = 25_000;

#[derive(Clone, Serialize, Type)]
pub struct DictationSessionStatus {
    /// "off", "listening" or "paused".
    pub state: String,
    pub error: Option<String>,
}

pub struct HandsFreeState {
    session: Mutex<HandsFreeSession>,
    /// Bumped on every Off/On so stale utterances are dropped.
    generation: std::sync::atomic::AtomicU64,
    error: Mutex<Option<String>>,
    utterances: Mutex<Option<mpsc::Sender<Utterance>>>,
}

struct Utterance {
    samples: Vec<f32>,
    timing: UtteranceTiming,
    generation: u64,
}

fn label(state: SessionState) -> &'static str {
    match state {
        SessionState::Off => "off",
        SessionState::Listening => "listening",
        SessionState::Paused => "paused",
    }
}

fn hf(app: &AppHandle) -> Option<tauri::State<'_, HandsFreeState>> {
    app.try_state::<HandsFreeState>()
}

/// Whether a session (listening or paused) is running. The wake listener
/// stands down while it is: the session hears "correct that" itself.
pub fn is_active(app: &AppHandle) -> bool {
    current(app) != SessionState::Off
}

fn current(app: &AppHandle) -> SessionState {
    hf(app)
        .and_then(|s| s.session.lock().ok().map(|s| s.state()))
        .unwrap_or(SessionState::Off)
}

fn status(app: &AppHandle) -> DictationSessionStatus {
    DictationSessionStatus {
        state: label(current(app)).into(),
        error: hf(app).and_then(|s| s.error.lock().ok().and_then(|e| e.clone())),
    }
}

/// Indicator, overlay and sound for entering `to`.
fn announce(app: &AppHandle, to: SessionState) {
    let _ = app.emit("dictation-session-state", status(app));
    match to {
        SessionState::Listening => {
            set_tray_state(app, TrayIconState::Recording);
            play_feedback_sound(app, SoundType::Start);
            crate::utils::show_feedback_overlay(app, "Listening", "success");
        }
        SessionState::Paused => {
            set_tray_state(app, TrayIconState::Idle);
            play_feedback_sound(app, SoundType::Stop);
            crate::utils::show_feedback_overlay(
                app,
                "Paused. Say \u{201C}unmute\u{201D} to continue.",
                "info",
            );
        }
        SessionState::Off => {
            set_tray_state(app, TrayIconState::Idle);
            // Two short stop chimes tell Off apart from Paused by ear.
            play_feedback_sound(app, SoundType::Stop);
            crate::utils::show_feedback_overlay(app, "Dictation off", "info");
            let later = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(220));
                play_feedback_sound(&later, SoundType::Stop);
            });
        }
    }
}

fn set_error(app: &AppHandle, error: Option<String>) {
    if let Some(s) = hf(app) {
        if let Ok(mut e) = s.error.lock() {
            *e = error;
        }
    }
}

/// Ctrl+Space (with hands-free mode on), the big button, or the tray.
pub fn toggle(app: &AppHandle) {
    let Some(s) = hf(app) else { return };
    let to = match s.session.lock() {
        Ok(mut session) => session.toggle().to,
        Err(_) => return,
    };
    s.generation.fetch_add(1, Ordering::SeqCst);
    set_error(app, None);
    if to == SessionState::Listening {
        if let Some(tm) = app.try_state::<Arc<TranscriptionManager>>() {
            tm.initiate_model_load();
        }
    } else if let Some(rm) = app.try_state::<Arc<AudioRecordingManager>>() {
        rm.stop_listener_capture();
    }
    info!("Hands-free dictation: {}", label(to));
    announce(app, to);
}

fn end_with_error(app: &AppHandle, error: String) {
    error!("Hands-free dictation stopped: {error}");
    if let Some(s) = hf(app) {
        if let Ok(mut session) = s.session.lock() {
            session.end();
        }
        s.generation.fetch_add(1, Ordering::SeqCst);
    }
    if let Some(rm) = app.try_state::<Arc<AudioRecordingManager>>() {
        rm.stop_listener_capture();
    }
    set_error(app, Some(error));
    announce(app, SessionState::Off);
}

pub fn init(app: &AppHandle) {
    let (tx, rx) = mpsc::channel::<Utterance>();
    app.manage(HandsFreeState {
        session: Mutex::new(HandsFreeSession::new()),
        generation: Default::default(),
        error: Mutex::new(None),
        utterances: Mutex::new(Some(tx)),
    });
    let segmenter = app.clone();
    std::thread::spawn(move || loop {
        if let Err(error) = segment(&segmenter) {
            end_with_error(&segmenter, error);
        }
        std::thread::sleep(Duration::from_millis(200));
    });
    let worker = app.clone();
    std::thread::spawn(move || {
        for utterance in rx {
            handle(&worker, utterance);
        }
    });
}

/// Simple endpointing on the raw VAD decision per frame.
struct Endpointer {
    frame: usize,
    voiced_run: usize,
    silent_run: usize,
    in_speech: bool,
    buffer: Vec<f32>,
    /// Samples since the last utterance ended (or the session started).
    gap: usize,
    /// Pre-roll samples at the start of the current utterance.
    lead: usize,
}

impl Endpointer {
    fn new(frame: usize) -> Self {
        Self {
            frame,
            voiced_run: 0,
            silent_run: 0,
            in_speech: false,
            buffer: Vec::new(),
            gap: 0,
            lead: 0,
        }
    }

    fn ms(samples: usize) -> u64 {
        (samples * 1000 / SAMPLE_RATE) as u64
    }

    fn frames(&self, ms: u64) -> usize {
        ((ms as usize * SAMPLE_RATE / 1000).div_ceil(self.frame)).max(1)
    }

    fn reset(&mut self) {
        *self = Self::new(self.frame);
    }

    /// Feed one VAD frame; returns a finished utterance.
    fn push(&mut self, frame: &[f32], voiced: bool) -> Option<(Vec<f32>, UtteranceTiming)> {
        self.buffer.extend_from_slice(frame);
        if !self.in_speech {
            self.voiced_run = if voiced { self.voiced_run + 1 } else { 0 };
            let pre_roll = self.frames(PRE_ROLL_MS) * self.frame;
            if self.buffer.len() > pre_roll + self.frame * self.voiced_run {
                let excess = self.buffer.len() - pre_roll - self.frame * self.voiced_run;
                self.buffer.drain(..excess);
                self.gap += excess;
            }
            if self.voiced_run >= self.frames(ONSET_MS) {
                self.in_speech = true;
                self.silent_run = 0;
                // The pre-roll was silence before this utterance.
                let pre = self.buffer.len() - self.frame * self.voiced_run;
                self.gap += pre;
                self.lead = pre;
            }
            return None;
        }
        self.silent_run = if voiced { 0 } else { self.silent_run + 1 };
        let ended = self.silent_run >= self.frames(END_SILENCE_MS);
        let too_long = Self::ms(self.buffer.len()) >= MAX_UTTERANCE_MS;
        if !ended && !too_long {
            return None;
        }
        let trailing = self.silent_run * self.frame;
        let samples = std::mem::take(&mut self.buffer);
        let timing = UtteranceTiming {
            silence_before_ms: Self::ms(self.gap),
            silence_after_ms: if ended { Self::ms(trailing) } else { 0 },
            duration_ms: Self::ms(samples.len().saturating_sub(trailing + self.lead)),
        };
        self.in_speech = false;
        self.voiced_run = 0;
        self.silent_run = 0;
        self.gap = trailing;
        Some((samples, timing))
    }
}

fn segment(app: &AppHandle) -> Result<(), String> {
    let rm = app.state::<Arc<AudioRecordingManager>>().inner().clone();
    let (tx, rx) = mpsc::sync_channel::<(u64, Vec<f32>)>(64);
    let overflow = Arc::new(AtomicBool::new(false));
    let mut vad: Option<Box<dyn crate::audio_toolkit::vad::SpeechScorer>> = None;
    let mut endpointer: Option<Endpointer> = None;
    let mut pending: Vec<f32> = Vec::new();
    let mut capture_generation = u64::MAX;
    let mut session_generation = u64::MAX;
    let mut last_frame = Instant::now();
    loop {
        let generation = hf(app)
            .ok_or("hands-free state missing")?
            .generation
            .load(Ordering::SeqCst);
        let state = current(app);
        let idle = app
            .try_state::<TranscriptionCoordinator>()
            .is_some_and(|c| c.is_idle());
        if state == SessionState::Off || !idle {
            // Off releases the microphone. A push-to-talk recording owns it
            // meanwhile; the session resumes listening afterwards.
            if state == SessionState::Off {
                vad = None;
            }
            rm.stop_listener_capture_for(capture_generation);
            endpointer.as_mut().map(Endpointer::reset);
            pending.clear();
            while rx.try_recv().is_ok() {}
            capture_generation = u64::MAX;
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        if vad.is_none() {
            let backend = get_settings(app).vad_backend;
            let detector = crate::managers::audio::create_detector(backend, app)
                .map_err(|e| format!("Could not start voice detection: {e}"))?;
            endpointer = Some(Endpointer::new(detector.frame_samples()));
            vad = Some(detector);
        }
        let epoch = rm.listener_epoch();
        let sender = tx.clone();
        let overrun = Arc::clone(&overflow);
        let callback: AudioFrameCallback = Arc::new(move |frame| {
            if sender
                .try_send((epoch.load(Ordering::Acquire), frame.to_vec()))
                .is_err()
            {
                overrun.store(true, Ordering::Release);
            }
        });
        match rm.start_listener_capture(callback) {
            Ok(g) if g != capture_generation => {
                capture_generation = g;
                while rx.try_recv().is_ok() {}
                pending.clear();
                endpointer.as_mut().map(Endpointer::reset);
                last_frame = Instant::now();
            }
            Ok(_) => {}
            Err(error) => {
                if !rm.is_recording() {
                    return Err(format!("The microphone could not start: {error}"));
                }
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        }
        if generation != session_generation {
            session_generation = generation;
            endpointer.as_mut().map(Endpointer::reset);
            pending.clear();
        }
        if overflow.swap(false, Ordering::AcqRel) {
            warn!("Hands-free audio queue overflowed; dropping the current utterance");
            endpointer.as_mut().map(Endpointer::reset);
            pending.clear();
            while rx.try_recv().is_ok() {}
        }
        let (epoch, frame) = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(f) => f,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_frame.elapsed() > Duration::from_secs(5) {
                    rm.stop_listener_capture_for(capture_generation);
                    return Err("The microphone stopped delivering audio. Check the input device and start dictation again.".into());
                }
                continue;
            }
            Err(_) => return Err("Microphone audio channel closed".into()),
        };
        if epoch != capture_generation || rm.listener_generation() != capture_generation {
            continue;
        }
        last_frame = Instant::now();
        let (Some(detector), Some(ep)) = (vad.as_mut(), endpointer.as_mut()) else {
            continue;
        };
        pending.extend_from_slice(&frame);
        let size = ep.frame;
        while pending.len() >= size {
            let chunk: Vec<f32> = pending.drain(..size).collect();
            let voiced = detector.is_voice(&chunk).unwrap_or(false);
            if let Some((samples, timing)) = ep.push(&chunk, voiced) {
                let gen = hf(app)
                    .map(|s| s.generation.load(Ordering::SeqCst))
                    .unwrap_or_default();
                // While paused, long speech never reaches the recognizer.
                let wanted = hf(app)
                    .and_then(|s| s.session.lock().ok().map(|s| s.should_transcribe(&timing)))
                    .unwrap_or(false);
                if !wanted {
                    debug!(
                        "Hands-free: dropped a {} ms utterance untranscribed",
                        timing.duration_ms
                    );
                    continue;
                }
                if let Some(s) = hf(app) {
                    if let Some(tx) = s.utterances.lock().ok().and_then(|t| t.clone()) {
                        let _ = tx.send(Utterance {
                            samples,
                            timing,
                            generation: gen,
                        });
                    }
                }
            }
        }
    }
}

fn handle(app: &AppHandle, utterance: Utterance) {
    let Some(s) = hf(app) else { return };
    let stale = || s.generation.load(Ordering::SeqCst) != utterance.generation;
    // Re-check against the state now (a "mute" may have been processed
    // since this utterance was cut).
    let wanted = s
        .session
        .lock()
        .ok()
        .is_some_and(|session| session.should_transcribe(&utterance.timing));
    if stale() || !wanted {
        return;
    }
    let Some(tm) = app.try_state::<Arc<TranscriptionManager>>() else {
        return;
    };
    let text = match tm.transcribe_recording(utterance.samples) {
        Ok(text) => text,
        Err(e) => {
            warn!("Hands-free transcription failed: {e}");
            let _ = app.emit("transcription-error", e.to_string());
            return;
        }
    };
    if stale() {
        return;
    }
    let action = match s.session.lock() {
        Ok(mut session) => session.on_utterance(&text, utterance.timing),
        Err(_) => return,
    };
    match action {
        UtteranceAction::Discard => {}
        UtteranceAction::Pause => announce(app, SessionState::Paused),
        UtteranceAction::Resume => announce(app, SessionState::Listening),
        UtteranceAction::Correct(instruction) => {
            if instruction.is_empty() {
                crate::utils::show_feedback_overlay(
                    app,
                    "Listening for a correction\u{2026}",
                    "info",
                );
                return;
            }
            let generation = app
                .state::<Arc<AudioRecordingManager>>()
                .cancel_generation();
            tauri::async_runtime::block_on(super::correction::handle_instruction_guarded(
                app.clone(),
                instruction,
                generation,
                false,
            ));
            // A correction may move text around: don't assume the next
            // utterance continues the last one.
            if let Ok(mut session) = s.session.lock() {
                session.reset_join();
            }
        }
        UtteranceAction::Insert(joined) => insert(app, &s, &text, joined, utterance.generation),
    }
}

/// Last place text was inserted, to restart joining after a focus change.
static LAST_TARGET: Mutex<Option<AppIdentity>> = Mutex::new(None);

fn insert(app: &AppHandle, s: &HandsFreeState, raw: &str, joined: String, generation: u64) {
    let target = focused_identity();
    let moved = LAST_TARGET
        .lock()
        .ok()
        .and_then(|last| last.clone())
        .is_some_and(|last| last.compare(&target) == TargetMatch::Different);
    let joined = if moved {
        // Another window: start fresh rather than joining onto the old one.
        if let Ok(mut session) = s.session.lock() {
            session.reset_join();
        }
        joined.trim_start_matches(' ').to_string()
    } else {
        joined
    };
    let processed = tauri::async_runtime::block_on(crate::actions::process_transcription_output(
        app,
        &joined,
        false,
        target.app_id.as_deref(),
    ));
    let text = processed.final_text;
    if text.is_empty() || s.generation.load(Ordering::SeqCst) != generation {
        return;
    }
    let (done_tx, done_rx) = mpsc::channel();
    let app_main = app.clone();
    let to_paste = text.clone();
    let raw = raw.to_string();
    let commit_target = target.clone();
    let scheduled = app.run_on_main_thread(move || {
        let pasted = match crate::clipboard::paste_exact(to_paste.clone(), app_main.clone()) {
            Ok(()) => true,
            Err(e) => {
                error!("Hands-free paste failed: {e}");
                let _ = app_main.emit("paste-error", ());
                false
            }
        };
        super::record_commit_exact(&app_main, &raw, &to_paste, commit_target, pasted);
        let _ = done_tx.send(pasted);
    });
    if scheduled.is_err() {
        return;
    }
    // Wait so utterances are inserted strictly in order.
    if done_rx.recv_timeout(Duration::from_secs(10)) == Ok(true) {
        if let Ok(mut session) = s.session.lock() {
            session.inserted(&text);
        }
        if let Ok(mut last) = LAST_TARGET.lock() {
            *last = Some(target);
        }
    }
}

#[tauri::command]
#[specta::specta]
pub fn get_dictation_session_status(app: AppHandle) -> DictationSessionStatus {
    status(&app)
}

#[tauri::command]
#[specta::specta]
pub fn toggle_dictation_session(app: AppHandle) -> DictationSessionStatus {
    toggle(&app);
    status(&app)
}

#[tauri::command]
#[specta::specta]
pub fn change_hands_free_shortcut_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.hands_free_shortcut = enabled;
    crate::settings::write_settings(&app, settings);
    if !enabled && is_active(&app) {
        toggle(&app);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(ep: &mut Endpointer, pattern: &[(bool, usize)]) -> Vec<UtteranceTiming> {
        let frame = vec![0.0f32; ep.frame];
        let mut out = Vec::new();
        for &(voiced, frames) in pattern {
            for _ in 0..frames {
                if let Some((_, t)) = ep.push(&frame, voiced) {
                    out.push(t);
                }
            }
        }
        out
    }

    #[test]
    fn utterances_end_at_a_pause_with_silence_measured_on_both_sides() {
        // 512-sample (32 ms) frames: 1 s silence, 0.5 s speech, 1 s silence.
        let mut ep = Endpointer::new(512);
        let got = feed(&mut ep, &[(false, 31), (true, 16), (false, 31)]);
        assert_eq!(got.len(), 1);
        let t = got[0];
        assert!(t.silence_before_ms >= 900, "{t:?}");
        assert!(t.silence_after_ms >= END_SILENCE_MS, "{t:?}");
        assert!((450..=600).contains(&t.duration_ms), "{t:?}");
    }

    #[test]
    fn a_click_does_not_start_an_utterance() {
        let mut ep = Endpointer::new(512);
        assert!(feed(&mut ep, &[(false, 10), (true, 1), (false, 40)]).is_empty());
    }

    #[test]
    fn back_to_back_utterances_report_the_short_gap() {
        let mut ep = Endpointer::new(512);
        let got = feed(
            &mut ep,
            &[
                (false, 31),
                (true, 20),
                (false, 23),
                (true, 20),
                (false, 31),
            ],
        );
        assert_eq!(got.len(), 2);
        // The second began ~740 ms after the first ended (the end silence
        // itself counts towards the gap).
        assert!(got[1].silence_before_ms >= END_SILENCE_MS, "{:?}", got[1]);
        assert!(got[1].silence_before_ms < 1000, "{:?}", got[1]);
    }

    #[test]
    fn monologues_are_cut() {
        let mut ep = Endpointer::new(512);
        let got = feed(&mut ep, &[(true, 1000)]);
        assert!(!got.is_empty());
        assert_eq!(got[0].silence_after_ms, 0);
    }
}
