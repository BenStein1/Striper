//! End-to-end check of the speech gate in front of the real Vosk library:
//! silence is never decoded, and "correct that" still comes out at the
//! utterance end. Needs the installed listener files, so it is ignored by
//! default:
//!
//! VOSK_LIB=…/libvosk.so VOSK_MODEL=…/vosk-model-small-en-us-0.15 \
//! WAKE_CLIPS=dir-of-16kHz-mono-16bit-wavs cargo test -- --ignored
//!
//! Each clip is named after what is said in it; clips whose name starts
//! with "correct_that" must wake, others must not.
use std::path::{Path, PathBuf};
use striper_listener::{wake_prefix, Detector, Gate, PreRoll, SpeechGate};

const FRAME: usize = 256;

fn read_wav(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    // Find the "data" chunk of a 16-bit mono PCM WAV.
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            return bytes[at + 8..(at + 8 + len).min(bytes.len())]
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                .collect();
        }
        at += 8 + len + (len & 1);
    }
    panic!("no data chunk in {}", path.display());
}

struct Outcome {
    endpoints: Vec<String>,
    decoded: usize,
    frames: usize,
}

fn run_gated(detector: &mut Detector, samples: &[f32]) -> Outcome {
    let mut vad = earshot::Detector::default_boxed();
    let mut gate = SpeechGate::new(FRAME, 60, 500, 1200);
    let mut audio = PreRoll::new(32_000);
    let mut out = Outcome {
        endpoints: Vec::new(),
        decoded: 0,
        frames: 0,
    };
    let mut voiced_since_endpoint = false;
    for chunk in samples.chunks_exact(FRAME) {
        out.frames += 1;
        let voiced = vad.predict_f32(chunk) >= 0.5;
        let event = gate.push(voiced);
        audio.push(chunk, event != Gate::Closed);
        if event == Gate::Closed {
            continue;
        }
        voiced_since_endpoint |= voiced;
        out.decoded += 1;
        let mut d = match event {
            Gate::Opened { frames } => detector.feed(audio.tail(frames * FRAME)).unwrap(),
            _ => detector.feed(chunk).unwrap(),
        };
        if !d.endpoint && event == Gate::Close {
            d = detector.end_utterance().unwrap();
            if !voiced_since_endpoint {
                d.text.clear();
            }
        }
        if d.endpoint {
            if !d.text.is_empty() {
                out.endpoints.push(d.text);
            }
            voiced_since_endpoint = false;
            // As the app does: the next candidate starts at this pause.
            audio.clear();
        }
    }
    out
}

#[test]
#[ignore = "needs VOSK_LIB, VOSK_MODEL and WAKE_CLIPS"]
fn gated_listener_hears_the_wake_phrase_and_skips_silence() {
    let var = |name| PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name}")));
    let mut detector = Detector::open(&var("VOSK_LIB"), &var("VOSK_MODEL")).unwrap();
    let mut clips: Vec<_> = std::fs::read_dir(var("WAKE_CLIPS"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    clips.sort();
    assert!(!clips.is_empty());
    for clip in clips {
        let name = clip.file_stem().unwrap().to_string_lossy().into_owned();
        let samples = read_wav(&clip);
        detector.reset();
        let out = run_gated(&mut detector, &samples);
        let woke = out.endpoints.iter().any(|t| wake_prefix(t).is_some());
        println!(
            "{name}: endpoints {:?}, decoded {}/{} frames",
            out.endpoints, out.decoded, out.frames
        );
        assert!(
            out.decoded < out.frames,
            "{name}: the gate decoded silence too"
        );
        assert_eq!(woke, name.starts_with("correct_that"), "{name}");
    }
}
