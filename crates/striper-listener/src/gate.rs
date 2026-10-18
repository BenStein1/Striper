//! When the wake listener may hold the microphone, and when to feed Vosk.
//! Pure frame counting and buffering; no audio, GUI or Vosk dependencies.
use std::collections::VecDeque;

const RATE: u64 = 16_000;

/// Whole detector frames covering `ms`, at least one.
pub fn frames_for(ms: u64, frame_samples: usize) -> usize {
    ((ms * RATE).div_ceil(frame_samples as u64 * 1000) as usize).max(1)
}

/// How long the listener holds the microphone after a push-to-talk
/// dictation or a voice-correction step. Long enough to reread a dictation
/// and say "correct that"; the correction shortcut works for longer.
pub const ARM_MS: u64 = 60_000;

/// A deadline in caller-supplied monotonic milliseconds, like `Conversation`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Arming {
    until: Option<u64>,
}

impl Arming {
    /// Listen until at least `now + ms`; never shortens an existing window.
    pub fn arm(&mut self, now: u64, ms: u64) {
        let until = now.saturating_add(ms);
        self.until = Some(self.until.map_or(until, |u| u.max(until)));
    }
    pub fn disarm(&mut self) {
        self.until = None;
    }
    pub fn armed(&self, now: u64) -> bool {
        self.until.is_some_and(|u| now < u)
    }
}

/// Everything that decides whether the wake listener holds the microphone.
#[derive(Debug, Clone, Copy, Default)]
pub struct Hold {
    /// "Listen for correct that" is on.
    pub enabled: bool,
    /// No push-to-talk recording or processing (the coordinator is idle).
    pub pipeline_idle: bool,
    /// A hands-free session (listening or paused) owns the microphone.
    pub hands_free: bool,
    /// Inside a window opened by a push-to-talk dictation or a correction.
    pub armed: bool,
    /// The ledger has a live dictation with a known window (initial wake).
    pub wake_target: bool,
    /// A follow-up was promised (resume pending) or is open.
    pub conversation: bool,
    /// The speech gate is open: never cut an utterance off mid-word.
    pub utterance_open: bool,
}

pub fn holds_mic(h: Hold) -> bool {
    h.enabled
        && h.pipeline_idle
        && !h.hands_free
        && (h.utterance_open || h.conversation || (h.armed && h.wake_target))
}

/// What to do with one VAD frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// No utterance: don't feed Vosk.
    Closed,
    /// An utterance began: feed the last `frames` frames (pre-roll and the
    /// voiced onset run, this frame included).
    Opened { frames: usize },
    /// Inside an utterance (or its hangover): feed this frame.
    Feed,
    /// Feed this frame, then end the utterance. The gate is closed after it.
    Close,
}

/// Opens on sustained speech and closes after a pause. Counts frames only;
/// the caller owns the audio.
#[derive(Debug, Clone)]
pub struct SpeechGate {
    frame_samples: usize,
    onset: usize,
    pre_roll: usize,
    hangover: usize,
    voiced_run: usize,
    silent_run: usize,
    open: bool,
    /// Frames since the gate opened. Vosk's endpoints don't reset it, so it
    /// bounds how long steady background speech can keep the gate open.
    open_frames: usize,
}

impl SpeechGate {
    pub fn new(frame_samples: usize, onset_ms: u64, pre_roll_ms: u64, hangover_ms: u64) -> Self {
        Self {
            frame_samples,
            onset: frames_for(onset_ms, frame_samples),
            pre_roll: frames_for(pre_roll_ms, frame_samples),
            hangover: frames_for(hangover_ms, frame_samples),
            voiced_run: 0,
            silent_run: 0,
            open: false,
            open_frames: 0,
        }
    }

    pub fn push(&mut self, voiced: bool) -> Gate {
        if !self.open {
            self.voiced_run = if voiced { self.voiced_run + 1 } else { 0 };
            if self.voiced_run < self.onset {
                return Gate::Closed;
            }
            self.open = true;
            self.silent_run = 0;
            self.open_frames = self.voiced_run;
            return Gate::Opened {
                frames: self.pre_roll + self.voiced_run,
            };
        }
        self.silent_run = if voiced { 0 } else { self.silent_run + 1 };
        self.open_frames += 1;
        if self.silent_run >= self.hangover {
            self.reset();
            return Gate::Close;
        }
        Gate::Feed
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// How long the gate has been open (0 while closed).
    pub fn open_ms(&self) -> u64 {
        self.open_frames as u64 * self.frame_samples as u64 * 1000 / RATE
    }

    pub fn reset(&mut self) {
        self.open = false;
        self.voiced_run = 0;
        self.silent_run = 0;
        self.open_frames = 0;
    }
}

/// Audio since the last utterance ended, for the selected recognizer and
/// the Vosk replay: trimmed to `keep` samples while nothing is open.
#[derive(Debug, Clone)]
pub struct PreRoll {
    buf: VecDeque<f32>,
    keep: usize,
}

impl PreRoll {
    pub fn new(keep: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(keep * 2),
            keep,
        }
    }
    pub fn push(&mut self, chunk: &[f32], keep_all: bool) {
        self.buf.extend(chunk.iter().copied());
        if !keep_all {
            let excess = self.buf.len().saturating_sub(self.keep);
            self.buf.drain(..excess);
        }
    }
    /// The newest `n` samples (fewer if not yet buffered), contiguous.
    pub fn tail(&mut self, n: usize) -> &[f32] {
        let all = self.buf.make_contiguous();
        &all[all.len().saturating_sub(n)..]
    }
    pub fn take(&mut self) -> Vec<f32> {
        self.buf.drain(..).collect()
    }
    pub fn clear(&mut self) {
        self.buf.clear();
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Silero frames: 512 samples, 32 ms.
    fn gate() -> SpeechGate {
        SpeechGate::new(512, 60, 500, 1200)
    }

    fn run(g: &mut SpeechGate, pattern: &[(bool, usize)]) -> Vec<Gate> {
        let mut out = Vec::new();
        for &(voiced, n) in pattern {
            for _ in 0..n {
                out.push(g.push(voiced));
            }
        }
        out
    }

    #[test]
    fn frame_counts_round_up() {
        assert_eq!(frames_for(60, 512), 2);
        assert_eq!(frames_for(500, 512), 16);
        assert_eq!(frames_for(1200, 512), 38);
        assert_eq!(frames_for(60, 256), 4);
        assert_eq!(frames_for(500, 256), 32);
        assert_eq!(frames_for(1200, 256), 75);
        assert_eq!(frames_for(0, 512), 1);
    }

    #[test]
    fn silence_never_opens() {
        let mut g = gate();
        assert!(run(&mut g, &[(false, 10_000)])
            .iter()
            .all(|e| *e == Gate::Closed));
    }

    #[test]
    fn a_click_does_not_open() {
        let mut g = gate();
        let events = run(&mut g, &[(false, 5), (true, 1), (false, 50)]);
        assert!(events.iter().all(|e| *e == Gate::Closed));
    }

    #[test]
    fn opens_with_pre_roll_and_closes_after_the_hangover() {
        let mut g = gate();
        let events = run(&mut g, &[(false, 40), (true, 20), (false, 38)]);
        assert_eq!(events[41], Gate::Opened { frames: 16 + 2 });
        assert!(events[42..97].iter().all(|e| *e == Gate::Feed));
        assert_eq!(events[97], Gate::Close);
        assert!(!g.is_open());
    }

    #[test]
    fn a_short_pause_keeps_one_utterance() {
        // "correct that ... change Thursday": a 600 ms pause stays open.
        let mut g = gate();
        let events = run(&mut g, &[(true, 20), (false, 19), (true, 20), (false, 38)]);
        let opens = events
            .iter()
            .filter(|e| matches!(e, Gate::Opened { .. }))
            .count();
        let closes = events.iter().filter(|e| **e == Gate::Close).count();
        assert_eq!((opens, closes), (1, 1));
        assert_eq!(*events.last().unwrap(), Gate::Close);
    }

    #[test]
    fn steady_background_speech_keeps_counting_open_time() {
        // A TV or a call: speech with pauses shorter than the hangover never
        // closes the gate, but the time it has been open keeps growing.
        let mut g = gate();
        for _ in 0..20 {
            run(&mut g, &[(true, 60), (false, 28)]);
        }
        assert!(g.is_open());
        assert_eq!(g.open_ms(), 20 * 88 * 32, "counted from the onset frames");
        g.reset();
        assert_eq!(g.open_ms(), 0);
    }

    #[test]
    fn reopens_for_the_next_utterance() {
        let mut g = gate();
        let events = run(
            &mut g,
            &[(true, 5), (false, 38), (false, 10), (true, 5), (false, 38)],
        );
        let opens = events
            .iter()
            .filter(|e| matches!(e, Gate::Opened { .. }))
            .count();
        assert_eq!(opens, 2);
    }

    #[test]
    fn arming_extends_but_never_shortens() {
        let mut a = Arming::default();
        assert!(!a.armed(0));
        a.arm(1_000, ARM_MS);
        assert!(a.armed(1_000 + ARM_MS - 1));
        assert!(!a.armed(1_000 + ARM_MS));
        a.arm(20_000, ARM_MS);
        assert!(a.armed(20_000 + ARM_MS - 1));
        a.arm(21_000, 1_000);
        assert!(
            a.armed(20_000 + ARM_MS - 1),
            "a shorter request must not shorten it"
        );
        a.disarm();
        assert!(!a.armed(21_001));
    }

    fn hold() -> Hold {
        Hold {
            enabled: true,
            pipeline_idle: true,
            armed: true,
            wake_target: true,
            ..Default::default()
        }
    }

    #[test]
    fn mic_is_held_only_inside_a_window_with_a_target() {
        assert!(holds_mic(hold()));
        assert!(
            !holds_mic(Hold {
                armed: false,
                ..hold()
            }),
            "window expired"
        );
        assert!(
            !holds_mic(Hold {
                wake_target: false,
                ..hold()
            }),
            "nothing to correct"
        );
        assert!(!holds_mic(Hold {
            enabled: false,
            ..hold()
        }));
        assert!(
            !holds_mic(Hold {
                pipeline_idle: false,
                ..hold()
            }),
            "PTT owns the mic"
        );
        assert!(
            !holds_mic(Hold {
                hands_free: true,
                ..hold()
            }),
            "hands-free owns it"
        );
    }

    #[test]
    fn conversation_and_open_utterances_outlive_the_window() {
        let idle = Hold {
            armed: false,
            wake_target: false,
            ..hold()
        };
        assert!(!holds_mic(idle));
        assert!(holds_mic(Hold {
            conversation: true,
            ..idle
        }));
        assert!(holds_mic(Hold {
            utterance_open: true,
            ..idle
        }));
        assert!(!holds_mic(Hold {
            utterance_open: true,
            hands_free: true,
            ..idle
        }));
    }

    #[test]
    fn pre_roll_is_bounded_until_an_utterance_opens() {
        let mut p = PreRoll::new(32_000);
        for _ in 0..200 {
            p.push(&[0.0; 512], false);
        }
        assert_eq!(p.len(), 32_000);
        for _ in 0..100 {
            p.push(&[1.0; 512], true);
        }
        assert_eq!(p.len(), 32_000 + 51_200);
        assert_eq!(p.tail(1024).len(), 1024);
        assert!(p.tail(1024).iter().all(|s| *s == 1.0));
        assert_eq!(p.take().len(), 83_200);
        assert!(p.is_empty());
    }
}
