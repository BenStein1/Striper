//! CPU-only wake detection. Vosk is loaded on demand, never linked into the
//! dictation engine. No audio, GUI, microphone or GPU dependencies.
use libloading::Library;
use std::ffi::{c_char, c_float, c_int, c_void, CStr, CString};
use std::path::Path;

/// A deliberately exact prefix. Quoted prose and phrases in the middle of a
/// sentence must not activate the editor. PTT audio never enters this module.
pub fn wake_prefix(text: &str) -> Option<&str> {
    let text = text.trim();
    let mut words = text.splitn(3, char::is_whitespace);
    if !words.next()?.eq_ignore_ascii_case("correct") {
        return None;
    }
    let second = words
        .next()?
        .trim_end_matches(['.', ',', ';', ':', '!', '?']);
    if !second.eq_ignore_ascii_case("that") {
        return None;
    }
    Some(
        words
            .next()
            .unwrap_or("")
            .trim_start_matches([' ', '.', ',', ';', ':']),
    )
}

/// Bounded conversational state, independent of the acoustic backend.
/// Times are monotonic milliseconds supplied by the caller.
#[derive(Default)]
pub struct Conversation {
    until: Option<u64>,
}
impl Conversation {
    pub fn active(&self, now: u64) -> bool {
        self.until.is_some_and(|until| now < until)
    }
    pub fn wake(&mut self, now: u64) {
        self.until = Some(now.saturating_add(30_000));
    }
    pub fn reset(&mut self) {
        self.until = None;
    }
}

type ModelNew = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type Free = unsafe extern "C" fn(*mut c_void);
type RecNew = unsafe extern "C" fn(*mut c_void, c_float, *const c_char) -> *mut c_void;
type Accept = unsafe extern "C" fn(*mut c_void, *const c_float, c_int) -> c_int;
type Json = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type Reset = unsafe extern "C" fn(*mut c_void);

pub struct Detection {
    pub text: String,
    pub endpoint: bool,
}

/// Owns both handles and the library that defines their destructors. Created
/// and used on one listener thread. Paths must name trusted installed assets.
pub struct Detector {
    model: *mut c_void,
    recognizer: *mut c_void,
    model_free: Free,
    rec_free: Free,
    accept: Accept,
    partial: Json,
    result: Json,
    reset_fn: Reset,
    _library: Library,
}
impl Detector {
    pub fn open(library: &Path, model: &Path) -> Result<Self, String> {
        let path =
            CString::new(model.to_string_lossy().as_bytes()).map_err(|_| "Invalid model path")?;
        // SAFETY: the caller selects an installed Vosk library; symbols match
        // vosk_api.h 0.3.45. Retaining Library keeps all copied pointers alive.
        unsafe {
            #[cfg(windows)]
            let library: Library = libloading::os::windows::Library::load_with_flags(
                library,
                0x00000100 | 0x00001000, // DLL_LOAD_DIR | DEFAULT_DIRS
            )
            .map_err(|e| e.to_string())?
            .into();
            #[cfg(not(windows))]
            let library = Library::new(library).map_err(|e| e.to_string())?;
            let model_new: ModelNew = *library
                .get(b"vosk_model_new\0")
                .map_err(|e| e.to_string())?;
            let model_free: Free = *library
                .get(b"vosk_model_free\0")
                .map_err(|e| e.to_string())?;
            let rec_new: RecNew = *library
                .get(b"vosk_recognizer_new_grm\0")
                .map_err(|e| e.to_string())?;
            let rec_free: Free = *library
                .get(b"vosk_recognizer_free\0")
                .map_err(|e| e.to_string())?;
            let accept: Accept = *library
                .get(b"vosk_recognizer_accept_waveform_f\0")
                .map_err(|e| e.to_string())?;
            let partial: Json = *library
                .get(b"vosk_recognizer_partial_result\0")
                .map_err(|e| e.to_string())?;
            let result: Json = *library
                .get(b"vosk_recognizer_result\0")
                .map_err(|e| e.to_string())?;
            let reset_fn: Reset = *library
                .get(b"vosk_recognizer_reset\0")
                .map_err(|e| e.to_string())?;
            let log_level: unsafe extern "C" fn(c_int) = *library
                .get(b"vosk_set_log_level\0")
                .map_err(|e| e.to_string())?;
            log_level(-1);
            let model = model_new(path.as_ptr());
            if model.is_null() {
                return Err("Could not load the wake-listener model".into());
            }
            // Include unknown speech explicitly; otherwise a tiny grammar can
            // force unrelated audio into the wake phrase. The selected ASR,
            // not this acoustic detector, transcribes the actual instruction.
            let grammar = c"[\"correct that\", \"[unk]\"]";
            let recognizer = rec_new(model, 16_000.0, grammar.as_ptr());
            if recognizer.is_null() {
                model_free(model);
                return Err("Could not create the wake listener".into());
            }
            Ok(Self {
                model,
                recognizer,
                model_free,
                rec_free,
                accept,
                partial,
                result,
                reset_fn,
                _library: library,
            })
        }
    }
    pub fn feed(&mut self, samples: &[f32]) -> Result<Detection, String> {
        if samples.len() > c_int::MAX as usize || samples.iter().any(|s| !s.is_finite()) {
            return Err("Invalid listener audio frame".into());
        }
        // Vosk's float API takes signed PCM16 magnitudes, not normalized f32.
        let pcm: Vec<f32> = samples
            .iter()
            .map(|s| s.clamp(-1.0, 1.0) * 32767.0)
            .collect();
        unsafe {
            let accepted = (self.accept)(self.recognizer, pcm.as_ptr(), pcm.len() as c_int);
            if accepted < 0 {
                return Err("Wake detector failed".into());
            }
            let endpoint = accepted != 0;
            let ptr = if endpoint {
                (self.result)(self.recognizer)
            } else {
                (self.partial)(self.recognizer)
            };
            if ptr.is_null() {
                return Err("Wake detector returned no result".into());
            }
            // Borrowed JSON belongs to Vosk and is copied before the next call.
            let json: serde_json::Value = serde_json::from_slice(CStr::from_ptr(ptr).to_bytes())
                .map_err(|e| e.to_string())?;
            let text = json[if endpoint { "text" } else { "partial" }]
                .as_str()
                .unwrap_or("")
                .to_owned();
            Ok(Detection { text, endpoint })
        }
    }
    pub fn reset(&mut self) {
        unsafe {
            (self.reset_fn)(self.recognizer);
        }
    }
}
impl Drop for Detector {
    fn drop(&mut self) {
        unsafe {
            (self.rec_free)(self.recognizer);
            (self.model_free)(self.model);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_command_prefix_only() {
        assert_eq!(
            wake_prefix("Correct that. Change Thursday to Friday"),
            Some("Change Thursday to Friday")
        );
        assert_eq!(wake_prefix("correct that"), Some(""));
        for prose in [
            "we should correct that",
            "correct that's wrong",
            "correct thatcher",
            "incorrect that",
            "\"correct that\" is a phrase",
        ] {
            assert_eq!(wake_prefix(prose), None, "{prose}");
        }
    }
    #[test]
    fn selected_recognizer_must_confirm_a_keyword_candidate() {
        // A constrained CPU decoder can hear these as "correct that".
        // The independent selected-ASR result is authoritative.
        assert_eq!(wake_prefix("Connect that cable."), None);
        assert_eq!(wake_prefix("Correct the spelling."), None);
        assert_eq!(wake_prefix("Directly that."), None);
        assert_eq!(wake_prefix("Correct that."), Some(""));
        assert_eq!(
            wake_prefix("Correct that, change Thursday to Friday."),
            Some("change Thursday to Friday.")
        );
    }

    #[test]
    fn sessions_expire_and_can_be_cancelled() {
        let mut c = Conversation::default();
        assert!(!c.active(0));
        c.wake(100);
        assert!(c.active(30_099));
        assert!(!c.active(30_100));
        c.wake(40_000);
        c.reset();
        assert!(!c.active(40_001));
    }
    #[test]
    fn missing_library_fails_without_affecting_dictation() {
        assert!(Detector::open(Path::new("/not-a-vosk-library"), Path::new("/no-model")).is_err());
    }
}
