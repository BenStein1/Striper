//! Opt-in, local CPU listener. Idle audio is never persisted or sent to an
//! endpoint. Vosk only detects the wake prefix and utterance boundaries;
//! correction audio uses the already-selected Handy recognizer.
use super::{focused_identity, now_ms, state};
use crate::audio_toolkit::audio::AudioFrameCallback;
use crate::audio_toolkit::vad::{SpeechScorer, VAD_ONSET_MS};
use crate::managers::audio::create_detector;
use crate::managers::{audio::AudioRecordingManager, transcription::TranscriptionManager};
use crate::settings::{get_settings, write_settings, VadBackend};
use crate::TranscriptionCoordinator;
use serde::Serialize;
use specta::Type;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use striper_core::session::{AppIdentity, TargetMatch};
use striper_listener::{
    holds_mic, wake_prefix, Arming, Conversation, Detector, Gate, Hold, PreRoll, SpeechGate, ARM_MS,
};
use tauri::{AppHandle, Emitter, Manager};

const MAX_AUDIO: usize = 20 * 16_000;
/// Audio kept before speech starts: replayed into Vosk at onset (the quiet
/// "cor-" of "correct" can sit below the VAD threshold) and given to the
/// selected recognizer with a candidate.
const PRE_ROLL_SAMPLES: usize = 2 * 16_000;
const GATE_PRE_ROLL_MS: u64 = 500;
/// Longer than the model's longest end-of-utterance silence rule (1 s), so
/// Vosk still splits utterances itself; the gate's close is only a backstop.
const GATE_HANGOVER_MS: u64 = 1200;
/// Past the window an open gate keeps the microphone only this long, so a
/// TV or a call in the room can't keep the listener running.
const UTTERANCE_HOLD_MS: u64 = 20_000;
const SETTINGS_REFRESH: Duration = Duration::from_millis(250);
const FOCUS_REFRESH: Duration = Duration::from_millis(250);

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

/// Monotonic milliseconds shared by the arming window and conversations.
fn mono_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// When the listener may hold the microphone. Push-to-talk dictations and
/// voice corrections open a short window; hands-free sessions, cancels and
/// the setting turning off close it. Outside a window the mic stays closed,
/// so an idle Striper costs nothing.
#[derive(Default)]
pub struct ListenerArm(Mutex<ArmSlot>);

#[derive(Default)]
struct ArmSlot {
    arming: Arming,
    /// A correction finished (or a bare "correct that" was confirmed) in
    /// this window: reopen the follow-up conversation there.
    resume: Option<AppIdentity>,
    /// Bumped by every change, so the loop drops a conversation that a new
    /// dictation, a hands-free toggle or a cancel has ended.
    version: u64,
}

impl ListenerArm {
    fn update(&self, f: impl FnOnce(&mut ArmSlot)) {
        if let Ok(mut slot) = self.0.lock() {
            f(&mut slot);
            slot.version = slot.version.wrapping_add(1);
        }
    }
    /// (armed, follow-up pending, version). A follow-up lapses with the
    /// window `resume_conversation` opened for it, so a stale one can never
    /// start a conversation later (after a mic failure, or the setting
    /// coming back on).
    fn snapshot(&self, now: u64) -> (bool, bool, u64) {
        self.0
            .lock()
            .map(|mut s| {
                let armed = s.arming.armed(now);
                if !armed {
                    s.resume = None;
                }
                (armed, s.resume.is_some(), s.version)
            })
            .unwrap_or_default()
    }
    fn take_resume(&self, now: u64) -> Option<AppIdentity> {
        let mut s = self.0.lock().ok()?;
        let resume = s.resume.take();
        resume.filter(|_| s.arming.armed(now))
    }
    fn after_dictation(&self, now: u64) {
        self.update(|s| s.arming.arm(now, ARM_MS));
    }
    fn resume_conversation(&self, target: AppIdentity, now: u64) {
        self.update(|s| {
            s.resume = Some(target);
            s.arming.arm(now, ARM_MS);
        });
    }
    fn end_conversation(&self) {
        self.update(|s| s.resume = None);
    }
    fn disarm(&self) {
        self.update(|s| {
            s.arming.disarm();
            s.resume = None;
        });
    }
}

fn arm(app: &AppHandle) -> Option<tauri::State<'_, ListenerArm>> {
    app.try_state::<ListenerArm>()
}

/// A push-to-talk dictation was inserted: listen for "correct that" briefly.
pub fn after_dictation(app: &AppHandle) {
    if let Some(a) = arm(app) {
        a.after_dictation(mono_ms());
    }
}

/// A correction step finished in `target`: offer a voice follow-up there.
fn resume_conversation(app: &AppHandle, target: AppIdentity) {
    if let Some(a) = arm(app) {
        a.resume_conversation(target, mono_ms());
    }
}

/// Any new dictation (push-to-talk or hands-free) ends a voice conversation.
pub fn end_conversation(app: &AppHandle) {
    if let Some(a) = arm(app) {
        a.end_conversation();
    }
}

/// Release the microphone now: a hands-free session started or ended, the
/// user cancelled, or the setting was turned off.
pub fn disarm(app: &AppHandle) {
    if let Some(a) = arm(app) {
        a.disarm();
    }
}

/// Whether there is a recent dictation with a known window that a wake
/// phrase could correct. Cheap (no focus probe).
fn armable(app: &AppHandle) -> bool {
    state(app).is_some_and(|s| {
        s.ledger.lock().is_ok_and(|ledger| {
            ledger.is_live(now_ms()) && ledger.target().is_some_and(|t| t.window_id.is_some())
        })
    })
}

/// Only wake against a known, recent dictation in the focused window. Unknown
/// Wayland focus uses the explicit shortcut rather than ambient activation.
fn target(app: &AppHandle) -> Option<AppIdentity> {
    // Probing focus can spawn a helper (kdotool on Wayland); skip it when
    // nothing could match anyway.
    if !armable(app) {
        return None;
    }
    let focused = super::focused_window_token();
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

/// Whether the focused window is still `target`, the window a follow-up
/// conversation belongs to. Unlike [`target`] this needs no live ledger, so
/// follow-ups also work after a correction applied to the live field text.
fn focus_matches(target: &AppIdentity) -> bool {
    let focused = super::focused_window_token();
    focused.window_id.is_some()
        && focused.window_id == target.window_id
        && target.compare(&focused) == TargetMatch::Same
}

/// Selected VAD for the speech gate; Earshot if Silero cannot load, so the
/// gate never disables voice correction.
fn create_gate_vad(app: &AppHandle, backend: VadBackend) -> Result<Box<dyn SpeechScorer>, String> {
    create_detector(backend, app)
        .or_else(|e| {
            log::warn!("Wake listener VAD {backend:?} unavailable ({e}); using Earshot");
            create_detector(VadBackend::Earshot, app)
        })
        .map_err(|e| format!("Could not start voice detection: {e}"))
}

/// Once a minute, what the listener (and the whole app) cost, so a report of
/// high idle CPU can be traced from the log. Quiet while the listener is on
/// standby and the app is idle, and while the listener is off or paused
/// (hands-free sessions and transcriptions are expected to cost CPU).
struct CostLog {
    since: Instant,
    cpu: Option<(Duration, Duration)>,
    held_ms: u64,
    frames: u64,
    decoded: u64,
    probes: u64,
}

impl CostLog {
    const PERIOD: Duration = Duration::from_secs(60);
    /// Log on standby when the whole app used more than this share of one
    /// core over the period.
    const BUSY_APP: f64 = 0.05;

    fn new() -> Self {
        Self {
            since: Instant::now(),
            cpu: cpu_times(),
            held_ms: 0,
            frames: 0,
            decoded: 0,
            probes: 0,
        }
    }

    fn tick(&mut self, phase: &str) {
        let wall = self.since.elapsed();
        if wall < Self::PERIOD {
            return;
        }
        let now = cpu_times();
        let share = |d: Duration| d.as_secs_f64() / wall.as_secs_f64();
        let (thread, app) = match (self.cpu, now) {
            (Some((t0, a0)), Some((t1, a1))) => (
                Some(share(t1.saturating_sub(t0))),
                Some(share(a1.saturating_sub(a0))),
            ),
            _ => (None, None),
        };
        if self.held_ms > 0 || (phase == "standby" && app.is_some_and(|a| a > Self::BUSY_APP)) {
            let pct = |v: Option<f64>| v.map_or("n/a".into(), |v| format!("{:.1}%", v * 100.0));
            log::info!(
                "Wake listener, last {}s: listener thread {} of a core, whole app {}; phase {phase}, mic held {}s, {} frames, {} sent to Vosk, {} focus checks",
                wall.as_secs(),
                pct(thread),
                pct(app),
                self.held_ms / 1000,
                self.frames,
                self.decoded,
                self.probes,
            );
        }
        *self = Self::new();
    }
}

/// CPU time used so far by (this thread, the whole process).
#[cfg(unix)]
fn cpu_times() -> Option<(Duration, Duration)> {
    fn clock(id: libc::clockid_t) -> Option<Duration> {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: clock_gettime only writes the timespec we pass.
        (unsafe { libc::clock_gettime(id, &mut ts) } == 0)
            .then(|| Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32))
    }
    Some((
        clock(libc::CLOCK_THREAD_CPUTIME_ID)?,
        clock(libc::CLOCK_PROCESS_CPUTIME_ID)?,
    ))
}

#[cfg(not(unix))]
fn cpu_times() -> Option<(Duration, Duration)> {
    None
}

/// Per-utterance state, cleared whenever an utterance ends or is abandoned.
struct Utterance {
    /// Audio since the last utterance ended (2 s of pre-roll while silent),
    /// for the selected recognizer and the Vosk replay at speech onset.
    audio: PreRoll,
    /// Skip an overlong stretch of speech until the next pause.
    discard: bool,
    /// Voiced frames since Vosk's last endpoint; a gate close without any is
    /// just the hangover after an endpoint.
    voiced_since_endpoint: bool,
    /// Vosk's partial result started with the wake phrase.
    woke: bool,
}

impl Utterance {
    fn new() -> Self {
        Self {
            audio: PreRoll::new(PRE_ROLL_SAMPLES),
            discard: false,
            voiced_since_endpoint: false,
            woke: false,
        }
    }
    fn clear(&mut self) {
        self.audio.clear();
        self.discard = false;
        self.voiced_since_endpoint = false;
        self.woke = false;
    }
}

fn run(app: &AppHandle) -> Result<(), String> {
    let rm = app.state::<Arc<AudioRecordingManager>>().inner().clone();
    let arm = app.state::<ListenerArm>();
    let (tx, rx) = mpsc::sync_channel::<(u64, Vec<f32>)>(64);
    let overflow = Arc::new(AtomicBool::new(false));
    let mut detector: Option<Detector> = None;
    let mut vad: Option<(VadBackend, Box<dyn SpeechScorer>, SpeechGate)> = None;
    let mut utterance = Utterance::new();
    // Samples not yet making a whole VAD frame. Not part of `Utterance`:
    // abandoning an utterance mid-delivery must not drop the frames after it.
    let mut pending: Vec<f32> = Vec::new();
    let mut conversation = Conversation::default();
    let mut session_target: Option<AppIdentity> = None;
    // Capture generation from start_listener_capture; u64::MAX = not held.
    let mut capture = u64::MAX;
    let mut restart_capture = true;
    let mut phase = String::new();
    let mut last_frame = Instant::now();
    let mut cancel_generation = rm.cancel_generation();
    let mut last_focus_probe: Option<Instant> = None;
    let mut cached_target: Option<AppIdentity> = None;
    let mut arm_seen = u64::MAX;
    // Reading settings deserializes the whole store, so refresh on a timer.
    let mut setting_enabled = false;
    let mut vad_backend = VadBackend::default();
    let mut setting_read: Option<Instant> = None;
    let mut cost = CostLog::new();
    let mut last_tick = Instant::now();
    loop {
        if capture != u64::MAX {
            cost.held_ms += last_tick.elapsed().as_millis() as u64;
        }
        last_tick = Instant::now();
        cost.tick(&phase);
        if setting_read.is_none_or(|at| at.elapsed() >= SETTINGS_REFRESH) {
            let settings = get_settings(app);
            setting_enabled = settings.correction_listener_enabled;
            vad_backend = settings.vad_backend;
            setting_read = Some(Instant::now());
        }
        // A hands-free session owns the microphone and hears "correct that"
        // itself; the wake listener stands down meanwhile.
        let enabled = setting_enabled && !super::hands_free::is_active(app);
        let now = mono_ms();
        let mut reset = false;
        if rm.cancel_generation() != cancel_generation {
            cancel_generation = rm.cancel_generation();
            disarm(app);
            reset = true;
        }
        let (armed, resume_pending, version) = arm.snapshot(now);
        if session_target.is_some() && !conversation.active(now) && !utterance.woke {
            // The follow-up ran out: the next wake needs a live dictation.
            session_target = None;
        }
        if version != arm_seen {
            // A new dictation, a hands-free toggle or a cancel ended any
            // conversation (a pending follow-up is picked up again below).
            arm_seen = version;
            reset = true;
        }
        if reset {
            utterance.clear();
            pending.clear();
            conversation.reset();
            session_target = None;
            if let Some(d) = detector.as_mut() {
                d.reset();
            }
            if let Some((_, v, gate)) = vad.as_mut() {
                v.reset();
                gate.reset();
            }
        }
        let idle = app
            .try_state::<TranscriptionCoordinator>()
            .is_some_and(|c| c.is_idle());
        let utterance_open = vad.as_ref().is_some_and(|(_, _, gate)| {
            gate.is_open() && !utterance.discard && gate.open_ms() < UTTERANCE_HOLD_MS
        });
        let hold = holds_mic(Hold {
            enabled,
            pipeline_idle: idle,
            hands_free: false,
            armed,
            // Only consulted inside the window; locks the ledger.
            wake_target: armed && armable(app),
            conversation: conversation.active(now) || resume_pending,
            utterance_open,
        });
        if !hold {
            // Nothing to correct, a dictation owns the mic, or the user is
            // done: release the microphone and decode nothing.
            if capture != u64::MAX {
                rm.stop_listener_capture_for(capture);
                capture = u64::MAX;
                utterance.clear();
                pending.clear();
                if let Some(d) = detector.as_mut() {
                    d.reset();
                }
                if let Some((_, v, gate)) = vad.as_mut() {
                    v.reset();
                    gate.reset();
                }
            }
            restart_capture = true;
            conversation.reset();
            session_target = None;
            if !setting_enabled {
                detector = None;
                vad = None;
            }
            while rx.try_recv().is_ok() {}
            let next = match (enabled, idle) {
                (false, _) => "off",
                (true, false) => "paused",
                (true, true) => "standby",
            };
            if phase != next {
                report(app, next, None);
                phase = next.into();
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        if detector.is_none() {
            report(app, "loading", None);
            phase = "loading".into();
            let loaded = assets(app).and_then(|(lib, model)| Detector::open(&lib, &model));
            match loaded {
                Ok(d) => detector = Some(d),
                Err(error) => {
                    report(app, "error", Some(error));
                    // Remain alive so toggling off/on can retry after setup.
                    while get_settings(app).correction_listener_enabled {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    phase.clear();
                    continue;
                }
            }
        }
        if vad
            .as_ref()
            .is_none_or(|(backend, _, _)| *backend != vad_backend)
        {
            let v = create_gate_vad(app, vad_backend)?;
            let gate = SpeechGate::new(
                v.frame_samples(),
                VAD_ONSET_MS,
                GATE_PRE_ROLL_MS,
                GATE_HANGOVER_MS,
            );
            vad = Some((vad_backend, v, gate));
            utterance.clear();
            pending.clear();
        }
        // Restart capture only when it was lost (preempted, closed, failed)
        // or frames stopped, not on every frame.
        if restart_capture || rm.listener_generation() != capture {
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
                Ok(g) => {
                    restart_capture = false;
                    if g != capture {
                        capture = g;
                        while rx.try_recv().is_ok() {}
                        utterance.clear();
                        pending.clear();
                        detector.as_mut().expect("loaded above").reset();
                        if let Some((_, v, gate)) = vad.as_mut() {
                            v.reset();
                            gate.reset();
                        }
                        conversation.reset();
                        session_target = None;
                        last_frame = Instant::now();
                    }
                }
                Err(error) => {
                    if !rm.is_recording() {
                        report(app, "error", Some(error));
                        phase = "error".into();
                    }
                    // No audio is coming: an open utterance must not keep
                    // the hold (and this retry) going past the window.
                    utterance.clear();
                    pending.clear();
                    if let Some((_, v, gate)) = vad.as_mut() {
                        v.reset();
                        gate.reset();
                    }
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
            }
        }
        // A finished correction reopens the conversation in its window, as
        // long as focus is still there.
        if resume_pending {
            if let Some(t) = arm.take_resume(now) {
                cost.probes += 1;
                if focus_matches(&t) {
                    conversation.wake(now);
                    session_target = Some(t);
                }
            }
        }
        if overflow.swap(false, Ordering::AcqRel) {
            utterance.clear();
            pending.clear();
            detector.as_mut().expect("loaded above").reset();
            if let Some((_, v, gate)) = vad.as_mut() {
                v.reset();
                gate.reset();
            }
            conversation.reset();
            session_target = None;
            while rx.try_recv().is_ok() {}
        }
        let next = if conversation.active(now) || utterance.woke {
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
                    rm.stop_listener_capture_for(capture);
                    return Err("The microphone stopped delivering audio. Check the input device and restart Striper.".into());
                }
                // Let start_listener_capture reopen a failed recorder.
                restart_capture = true;
                continue;
            }
            Err(_) => return Err("Microphone audio channel closed".into()),
        };
        if rm.cancel_generation() != cancel_generation
            || epoch != capture
            || rm.listener_generation() != capture
        {
            continue;
        }
        last_frame = Instant::now();
        cost.frames += 1;
        let (Some(detector), Some((_, vad, gate))) = (detector.as_mut(), vad.as_mut()) else {
            continue;
        };
        pending.extend_from_slice(&frame);
        let size = vad.frame_samples();
        let whole = pending.len() / size * size;
        for chunk in pending[..whole].chunks_exact(size) {
            let voiced = vad.is_voice(chunk).unwrap_or(false);
            let event = gate.push(voiced);
            utterance
                .audio
                .push(chunk, event != Gate::Closed && !utterance.discard);
            if utterance.discard {
                // Overlong speech: wait for a pause before listening again.
                utterance.discard = event != Gate::Close;
                if !utterance.discard {
                    utterance.audio.clear();
                }
                continue;
            }
            if event == Gate::Closed {
                // Silence: no Vosk, no focus checks.
                continue;
            }
            if utterance.audio.len() > MAX_AUDIO {
                // Bound memory and reject truncated/overlong utterances.
                utterance.clear();
                utterance.discard = gate.is_open();
                detector.reset();
                conversation.reset();
                session_target = None;
                continue;
            }
            utterance.voiced_since_endpoint |= voiced;
            cost.decoded += 1;
            let mut detection = match event {
                Gate::Opened { frames } => detector.feed(utterance.audio.tail(frames * size))?,
                _ => detector.feed(chunk)?,
            };
            if !detection.endpoint && event == Gate::Close {
                // The gate's backstop for Vosk's own endpoint.
                detection = detector.end_utterance()?;
                if !utterance.voiced_since_endpoint {
                    detection.text.clear();
                }
            }
            // Focus matters only for a possible wake phrase or an open
            // conversation; ordinary speech triggers no probes.
            let needs_focus = utterance.woke
                || session_target.is_some()
                || conversation.active(now)
                || mentions_wake_word(&detection.text);
            if needs_focus && last_focus_probe.is_none_or(|at| at.elapsed() >= FOCUS_REFRESH) {
                // This only gates acoustic candidates. Before editing, the
                // async handler and each input operation check focus again.
                cost.probes += 1;
                cached_target = match &session_target {
                    Some(t) => focus_matches(t).then(|| t.clone()),
                    None => target(app),
                };
                last_focus_probe = Some(Instant::now());
            } else if !needs_focus {
                cached_target = None;
                last_focus_probe = None;
            }
            let current = cached_target.clone();
            if session_target.is_some() && current.is_none() {
                // Focus moved away from the conversation's window.
                conversation.reset();
                session_target = None;
                utterance.clear();
                utterance.discard = gate.is_open();
                detector.reset();
                continue;
            }
            if current.is_some() && wake_prefix(&detection.text).is_some() && !utterance.woke {
                session_target = current.clone();
                utterance.woke = true;
                // No chime here: speaker feedback could contaminate same-breath
                // instructions. The tray/overlay provides a visual acknowledgment.
                crate::utils::show_feedback_overlay(
                    app,
                    "Checking correction wake phrase…",
                    "info",
                );
            }
            if detection.endpoint {
                // CPU output is only a candidate. The selected recognizer must
                // confirm an initial wake, including a standalone wake phrase.
                let require_wake = !conversation.active(now);
                let candidate = wake_prefix(&detection.text).is_some()
                    || (!require_wake && !detection.text.trim().is_empty());
                if candidate && current.is_some() {
                    let samples = utterance.audio.take();
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
                    // Not for us. Inside the gate everything after this is
                    // kept, so the next candidate starts at this pause rather
                    // than with the words just spoken.
                    utterance.audio.clear();
                }
                if !conversation.active(now) {
                    session_target = None;
                }
                utterance.woke = false;
                utterance.voiced_since_endpoint = false;
            }
        }
        pending.drain(..whole);
    }
}

/// Whether Vosk's text could be (the start of) the wake phrase. Its grammar
/// only knows "correct", "that" and unknown speech.
fn mentions_wake_word(text: &str) -> bool {
    text.split_whitespace()
        .any(|w| w.eq_ignore_ascii_case("correct"))
}

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
                        resume_conversation(&app, original);
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
                if open && get_settings(&app).correction_listener_enabled {
                    resume_conversation(&app, original);
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
    if !enabled {
        disarm(&app);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: &str) -> AppIdentity {
        AppIdentity {
            app_id: Some("kate".into()),
            window_id: Some(id.into()),
            title: None,
        }
    }

    #[test]
    fn push_to_talk_arms_the_listener_for_a_window() {
        let arm = ListenerArm::default();
        let (armed, resume, v0) = arm.snapshot(0);
        assert!(!armed && !resume);
        arm.after_dictation(1_000);
        let (armed, resume, v1) = arm.snapshot(1_000 + ARM_MS - 1);
        assert!(armed && !resume);
        assert_ne!(v0, v1, "every change is visible to the loop");
        assert!(!arm.snapshot(1_000 + ARM_MS).0, "then the mic closes");
    }

    #[test]
    fn stopping_dictation_or_cancelling_releases_the_microphone() {
        let arm = ListenerArm::default();
        arm.after_dictation(0);
        arm.resume_conversation(window("1"), 0);
        arm.disarm();
        let (armed, resume, _) = arm.snapshot(1);
        assert!(!armed && !resume);
        assert!(arm.take_resume(1).is_none());
    }

    #[test]
    fn a_new_dictation_drops_a_pending_follow_up() {
        // Otherwise the next utterance in that window would be treated as an
        // instruction without "correct that".
        let arm = ListenerArm::default();
        arm.resume_conversation(window("1"), 0);
        assert!(arm.snapshot(1).1);
        arm.end_conversation();
        let (armed, resume, _) = arm.snapshot(1);
        assert!(armed, "the dictation's own window stays");
        assert!(!resume);
    }

    #[test]
    fn a_follow_up_is_taken_once() {
        let arm = ListenerArm::default();
        arm.resume_conversation(window("7"), 0);
        assert_eq!(arm.take_resume(1), Some(window("7")));
        assert_eq!(arm.take_resume(1), None);
    }

    #[test]
    fn a_follow_up_lapses_with_its_window() {
        // E.g. the mic failed, or the setting went off and came back on
        // later: the next utterance must not be taken as an instruction.
        let arm = ListenerArm::default();
        arm.resume_conversation(window("7"), 0);
        assert!(!arm.snapshot(ARM_MS).1);
        assert_eq!(arm.take_resume(ARM_MS), None);
        arm.resume_conversation(window("8"), 0);
        assert_eq!(arm.take_resume(ARM_MS), None, "even without a snapshot");
    }

    #[test]
    fn only_the_wake_word_triggers_focus_checks() {
        assert!(mentions_wake_word("[unk] correct that"));
        assert!(mentions_wake_word("Correct"));
        assert!(!mentions_wake_word("[unk] [unk]"));
        assert!(!mentions_wake_word("that"));
        assert!(!mentions_wake_word(""));
    }
}
