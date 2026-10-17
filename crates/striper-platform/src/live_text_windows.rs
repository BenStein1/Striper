//! Windows UI Automation adapter for [`super`] (PLAN.md §9a).
//!
//! Reads the focused element through `IUIAutomationTextPattern` (document
//! text, selection, caret) and falls back to `IUIAutomationValuePattern` for
//! simple fields. Selection goes through `IUIAutomationTextRange::Select`.
//! Nothing here writes text.
//!
//! Providers disagree on what a `TextUnit_Character` is (UTF-16 unit,
//! grapheme, one unit or two for `\r\n`), so offsets are never computed by
//! counting: they come from the text of real ranges, and movements are
//! corrected by reading the moved range back (see [`extend_to_text`]).
use super::*;

use windows::core::{Interface, BOOL, BSTR};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    SAFEARRAY,
};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationTextPattern2, IUIAutomationTextRange, IUIAutomationValuePattern,
    TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, TextUnit_Character,
    UIA_ComboBoxControlTypeId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_TextPattern2Id, UIA_TextPatternId, UIA_ValuePatternId,
};

/// Upper bound on read-back corrections when moving an endpoint to match a
/// known text. Each round fixes the length mismatch the provider reported.
const MAX_CORRECTIONS: usize = 12;

/// Keep a hung target app from stalling the caller for UIA's default 20 s.
const CONNECTION_TIMEOUT_MS: u32 = 1_000;
const TRANSACTION_TIMEOUT_MS: u32 = 2_000;

pub fn read_focused_text(max_chars: usize) -> Result<FocusedText, LiveTextError> {
    // Declared first so it is dropped last, after every COM object below.
    let _com = ComGuard::init()?;
    let element = focused_element()?;
    read_element(&element, max_chars)
}

pub fn select_range(snapshot: &FocusedText, range: Range<usize>) -> Result<(), LiveTextError> {
    if !snapshot.can_select {
        return Err(LiveTextError::Unsupported(
            "this field does not support selection".into(),
        ));
    }
    let Some(expected_id) = snapshot.control_id.as_deref() else {
        return Err(LiveTextError::Failed(
            "the snapshot has no control identity".into(),
        ));
    };
    let _com = ComGuard::init()?;
    let element = focused_element()?;
    if runtime_id(&element).as_deref() != Some(expected_id) {
        return Err(LiveTextError::FocusChanged);
    }
    let pattern = text_pattern(&element).ok_or_else(|| {
        LiveTextError::Unsupported("the field has no UI Automation text pattern".into())
    })?;
    // SAFETY: COM call on a live interface pointer we own.
    let document = unsafe { pattern.DocumentRange() }.map_err(failed("DocumentRange"))?;

    let window_start = if snapshot.whole_document {
        collapsed(&document, TextPatternRangeEndpoint_Start)?
    } else {
        locate_window(&element, &pattern, &document, snapshot)?
    };

    let prefix = extend_to_text(&window_start, &snapshot.text[..range.start])
        .ok_or_else(|| LiveTextError::Failed("the field text changed".into()))?;
    let target_start = collapsed(&prefix, TextPatternRangeEndpoint_End)?;
    let expected = &snapshot.text[range];
    let target = extend_to_text(&target_start, expected)
        .ok_or_else(|| LiveTextError::Failed("the field text changed".into()))?;
    // SAFETY: COM call on a live interface pointer we own.
    unsafe { target.Select() }.map_err(failed("Select"))?;

    let selected = selection_range(&element, &pattern)
        .and_then(|r| range_text(&r, to_i32(utf16_len(expected).saturating_add(256))))
        .ok_or_else(|| LiveTextError::Failed("could not read the selection back".into()))?;
    if normalise_newlines(&selected) != normalise_newlines(expected) {
        return Err(LiveTextError::Failed("selection did not match".into()));
    }
    Ok(())
}

/// Balances a successful `CoInitializeEx` on this thread. An apartment of the
/// other kind (`RPC_E_CHANGED_MODE`) is used as is and left alone.
struct ComGuard {
    uninit: bool,
}

impl ComGuard {
    fn init() -> Result<Self, LiveTextError> {
        // SAFETY: initialises COM for the calling thread only; balanced in Drop.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_ok() {
            // S_OK and S_FALSE both need a matching CoUninitialize.
            Ok(ComGuard { uninit: true })
        } else if hr == RPC_E_CHANGED_MODE {
            Ok(ComGuard { uninit: false })
        } else {
            Err(LiveTextError::Failed(format!("CoInitializeEx: {hr:?}")))
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninit {
            // SAFETY: matches the successful CoInitializeEx in `init`.
            unsafe { CoUninitialize() };
        }
    }
}

fn failed(what: &'static str) -> impl Fn(windows::core::Error) -> LiveTextError {
    move |e| LiveTextError::Failed(format!("{what}: {e}"))
}

fn focused_element() -> Result<IUIAutomationElement, LiveTextError> {
    // SAFETY: creates the in-process UIA client object; COM is initialised.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(failed("CoCreateInstance(CUIAutomation)"))?;
    if let Ok(automation2) = automation.cast::<IUIAutomation2>() {
        // SAFETY: plain setters on a live interface; failure is harmless.
        unsafe {
            let _ = automation2.SetConnectionTimeout(CONNECTION_TIMEOUT_MS);
            let _ = automation2.SetTransactionTimeout(TRANSACTION_TIMEOUT_MS);
        }
    }
    // SAFETY: COM call on a live interface; a null result maps to Err.
    unsafe { automation.GetFocusedElement() }.map_err(|_| LiveTextError::NoTextField)
}

fn text_pattern(element: &IUIAutomationElement) -> Option<IUIAutomationTextPattern> {
    // SAFETY: COM call; an unsupported pattern yields null, mapped to Err.
    unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }.ok()
}

fn read_element(
    element: &IUIAutomationElement,
    max_chars: usize,
) -> Result<FocusedText, LiveTextError> {
    // SAFETY: COM property getter on a live interface.
    if unsafe { element.CurrentIsPassword() }.is_ok_and(|b| b.as_bool()) {
        return Err(LiveTextError::Protected);
    }
    let control_id = runtime_id(element);

    if let Some(pattern) = text_pattern(element) {
        let mut snapshot = read_text_pattern(element, &pattern, max_chars)?;
        snapshot.control_id = control_id;
        return Ok(snapshot);
    }

    // SAFETY: COM property getter on a live interface.
    let control_type = unsafe { element.CurrentControlType() }.ok();
    let text_like = control_type.is_some_and(|t| {
        t == UIA_EditControlTypeId
            || t == UIA_DocumentControlTypeId
            || t == UIA_ComboBoxControlTypeId
    });
    if !text_like {
        // Links, list items and the like expose ValuePattern too, but their
        // value is not text the user is editing.
        return Err(LiveTextError::NoTextField);
    }

    // SAFETY: COM call; an unsupported pattern yields null, mapped to Err.
    let Ok(value) =
        (unsafe { element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) })
    else {
        return Err(LiveTextError::Unsupported(
            "the field exposes neither a text nor a value pattern".into(),
        ));
    };
    // SAFETY: COM property getters on a live interface.
    let raw = unsafe { value.CurrentValue() }.map_err(failed("ValuePattern.CurrentValue"))?;
    let text = String::from_utf16_lossy(&raw);
    let (text, whole_document) = truncate_chars(text, max_chars);
    Ok(FocusedText {
        text,
        selection: None,
        whole_document,
        control_id,
        // ValuePattern has no selection API, read-only or not.
        can_select: false,
        source: "uia-value",
    })
}

fn read_text_pattern(
    element: &IUIAutomationElement,
    pattern: &IUIAutomationTextPattern,
    max_chars: usize,
) -> Result<FocusedText, LiveTextError> {
    // SAFETY: COM call on a live interface pointer we own.
    let document = unsafe { pattern.DocumentRange() }.map_err(failed("DocumentRange"))?;
    let limit = i32::try_from(max_chars.saturating_add(1)).unwrap_or(i32::MAX);
    let doc_text = range_text(&document, limit)
        .ok_or_else(|| LiveTextError::Failed("could not read the document text".into()))?;
    let selection = selection_range(element, pattern);

    if utf16_len(&doc_text) <= max_chars {
        let offsets = selection
            .as_ref()
            .and_then(|sel| selection_offsets(&document, &doc_text, sel));
        return Ok(FocusedText {
            text: doc_text,
            selection: offsets,
            whole_document: true,
            control_id: None,
            can_select: true,
            source: "uia-text",
        });
    }

    // Larger than requested: a window around the selection (or the start of
    // the document when the provider reports no selection or caret).
    let anchor = match &selection {
        Some(sel) => sel.clone(),
        None => collapsed(&document, TextPatternRangeEndpoint_Start)?,
    };
    let window = window_around(&anchor, max_chars)?;
    let text = range_text(&window, -1)
        .ok_or_else(|| LiveTextError::Failed("could not read the text window".into()))?;
    let offsets = selection
        .as_ref()
        .and_then(|sel| selection_offsets(&window, &text, sel));
    Ok(FocusedText {
        text,
        selection: offsets,
        whole_document: false,
        control_id: None,
        can_select: true,
        source: "uia-text",
    })
}

/// The first selected range, or the caret from TextPattern2 when the
/// provider reports no selection.
fn selection_range(
    element: &IUIAutomationElement,
    pattern: &IUIAutomationTextPattern,
) -> Option<IUIAutomationTextRange> {
    // SAFETY: COM calls on live interfaces; index 0 is checked against Length.
    let first = unsafe {
        pattern.GetSelection().ok().and_then(|ranges| {
            if ranges.Length().ok()? > 0 {
                ranges.GetElement(0).ok()
            } else {
                None
            }
        })
    };
    first.or_else(|| {
        // SAFETY: COM call; an unsupported pattern yields null, mapped to Err.
        let pattern2 =
            unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id) }
                .ok()?;
        let mut active = BOOL::default();
        // SAFETY: COM call; `active` outlives the call.
        unsafe { pattern2.GetCaretRange(&mut active) }.ok()
    })
}

/// A window of about `max_chars` around `anchor`. Budget a side cannot use
/// because it hits the document edge goes to the other side.
fn window_around(
    anchor: &IUIAutomationTextRange,
    max_chars: usize,
) -> Result<IUIAutomationTextRange, LiveTextError> {
    let limit = i32::try_from(max_chars.saturating_add(1)).unwrap_or(i32::MAX);
    let anchor_len = range_text(anchor, limit).map_or(0, |t| utf16_len(&t));
    if anchor_len > max_chars {
        // The selection alone is too long: keep its first `max_chars`.
        let start = collapsed(anchor, TextPatternRangeEndpoint_Start)?;
        move_endpoint(&start, TextPatternRangeEndpoint_End, to_i32(max_chars))?;
        return Ok(start);
    }
    let window = clone_range(anchor)?;
    let budget = max_chars - anchor_len;
    let before = budget / 2;
    let after = budget - before;
    let moved_back = move_endpoint(&window, TextPatternRangeEndpoint_Start, -to_i32(before))?;
    let unused_before = before.saturating_sub(moved_back.unsigned_abs() as usize);
    let want_after = after + unused_before;
    let moved_fwd = move_endpoint(&window, TextPatternRangeEndpoint_End, to_i32(want_after))?;
    let unused_after = want_after.saturating_sub(moved_fwd.max(0) as usize);
    if unused_after > 0 {
        move_endpoint(
            &window,
            TextPatternRangeEndpoint_Start,
            -to_i32(unused_after),
        )?;
    }
    Ok(window)
}

/// Byte offsets of `sel` inside `base`, whose text is `base_text`. `None`
/// when the provider's texts do not line up (e.g. embedded objects).
fn selection_offsets(
    base: &IUIAutomationTextRange,
    base_text: &str,
    sel: &IUIAutomationTextRange,
) -> Option<Range<usize>> {
    let prefix = clone_range(base).ok()?;
    // SAFETY: COM calls on live interfaces we own.
    unsafe {
        // A selection outside the base range has no offsets in its text.
        if sel
            .CompareEndpoints(
                TextPatternRangeEndpoint_Start,
                base,
                TextPatternRangeEndpoint_Start,
            )
            .ok()?
            < 0
            || sel
                .CompareEndpoints(
                    TextPatternRangeEndpoint_End,
                    base,
                    TextPatternRangeEndpoint_End,
                )
                .ok()?
                > 0
        {
            return None;
        }
        prefix
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                sel,
                TextPatternRangeEndpoint_Start,
            )
            .ok()?;
    }
    let before = range_text(&prefix, -1)?;
    let selected = range_text(sel, -1)?;
    let start = before.len();
    let end = start.checked_add(selected.len())?;
    (base_text.get(..start) == Some(before.as_str())
        && base_text.get(start..end) == Some(selected.as_str()))
    .then_some(start..end)
}

/// Find where `snapshot.text` starts in the current document: first from the
/// current selection (cheap, usually right), then by a unique `FindText`.
fn locate_window(
    element: &IUIAutomationElement,
    pattern: &IUIAutomationTextPattern,
    document: &IUIAutomationTextRange,
    snapshot: &FocusedText,
) -> Result<IUIAutomationTextRange, LiveTextError> {
    if let (Some(sel), Some(snap_sel)) = (
        selection_range(element, pattern),
        snapshot.selection.as_ref(),
    ) {
        if let Some(start) = window_start_from_selection(&sel, &snapshot.text, snap_sel.start) {
            return Ok(start);
        }
    }
    let total = snapshot.text.chars().count();
    for want in [64usize, 512, total] {
        let n = want.min(total);
        if n == 0 {
            break;
        }
        let end = snapshot
            .text
            .char_indices()
            .nth(n)
            .map_or(snapshot.text.len(), |(i, _)| i);
        let needle = &snapshot.text[..end];
        match find_unique(document, needle) {
            Found::Unique(found) => {
                let start = collapsed(&found, TextPatternRangeEndpoint_Start)?;
                if extend_to_text(&start, &snapshot.text).is_some() {
                    return Ok(start);
                }
                break;
            }
            Found::Ambiguous if n < total => continue,
            _ => break,
        }
    }
    Err(LiveTextError::Failed(
        "could not locate the text window in the field".into(),
    ))
}

/// Walk back from the current selection start by the snapshot's prefix and
/// check that the whole snapshot text follows from there.
fn window_start_from_selection(
    sel: &IUIAutomationTextRange,
    text: &str,
    sel_start: usize,
) -> Option<IUIAutomationTextRange> {
    let prefix = text.get(..sel_start)?;
    let start = collapsed(sel, TextPatternRangeEndpoint_Start).ok()?;
    let back = utf16_len(prefix);
    if back > 0 {
        move_endpoint(&start, TextPatternRangeEndpoint_Start, -to_i32(back)).ok()?;
        let start = collapsed(&start, TextPatternRangeEndpoint_Start).ok()?;
        return extend_to_text(&start, text).map(|_| start);
    }
    extend_to_text(&start, text).map(|_| start)
}

enum Found {
    Unique(IUIAutomationTextRange),
    Ambiguous,
    Missing,
}

fn find_unique(document: &IUIAutomationTextRange, needle: &str) -> Found {
    let bstr = BSTR::from(needle);
    // SAFETY: COM call; `bstr` outlives it. Null (no match) maps to Err.
    let Ok(found) = (unsafe { document.FindText(&bstr, false, false) }) else {
        return Found::Missing;
    };
    let Ok(rest) = clone_range(document) else {
        return Found::Missing;
    };
    // SAFETY: COM calls on live interfaces we own.
    let again = unsafe {
        rest.MoveEndpointByRange(
            TextPatternRangeEndpoint_Start,
            &found,
            TextPatternRangeEndpoint_Start,
        )
        .and_then(|()| {
            rest.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, 1)
        })
        .and_then(|_| rest.FindText(&bstr, false, false))
    };
    if again.is_ok() {
        Found::Ambiguous
    } else {
        Found::Unique(found)
    }
}

/// A range starting where `start` starts and covering exactly `expected`,
/// or `None` when the field's text there is different.
///
/// The first move assumes one unit per UTF-16 code unit; every following
/// move corrects by the length difference the provider's own text shows.
/// That converges for providers counting `\r\n` as one unit (RichEdit, Word)
/// or graphemes as one unit (browsers) without knowing which one this is.
fn extend_to_text(
    start: &IUIAutomationTextRange,
    expected: &str,
) -> Option<IUIAutomationTextRange> {
    let range = collapsed(start, TextPatternRangeEndpoint_Start).ok()?;
    if expected.is_empty() {
        return Some(range);
    }
    let target = utf16_len(expected);
    let read_limit = to_i32(target.saturating_add(256));
    let mut step = to_i32(target);
    for _ in 0..MAX_CORRECTIONS {
        let moved = move_endpoint(&range, TextPatternRangeEndpoint_End, step).ok()?;
        if moved == 0 {
            return None;
        }
        let text = range_text(&range, read_limit)?;
        if text == expected {
            return Some(range);
        }
        let diff = target as i64 - utf16_len(&text) as i64;
        if diff == 0 {
            // Same length, different text: the field changed.
            return None;
        }
        step = i32::try_from(diff).ok()?;
    }
    None
}

/// A degenerate range at `endpoint` of `range`.
fn collapsed(
    range: &IUIAutomationTextRange,
    endpoint: windows::Win32::UI::Accessibility::TextPatternRangeEndpoint,
) -> Result<IUIAutomationTextRange, LiveTextError> {
    let point = clone_range(range)?;
    let other = if endpoint == TextPatternRangeEndpoint_Start {
        TextPatternRangeEndpoint_End
    } else {
        TextPatternRangeEndpoint_Start
    };
    // SAFETY: COM call on live interfaces we own.
    unsafe { point.MoveEndpointByRange(other, range, endpoint) }
        .map_err(failed("MoveEndpointByRange"))?;
    Ok(point)
}

fn clone_range(range: &IUIAutomationTextRange) -> Result<IUIAutomationTextRange, LiveTextError> {
    // SAFETY: COM call on a live interface pointer we own.
    unsafe { range.Clone() }.map_err(failed("TextRange.Clone"))
}

/// Move one endpoint by `count` characters; returns how far it moved.
fn move_endpoint(
    range: &IUIAutomationTextRange,
    endpoint: windows::Win32::UI::Accessibility::TextPatternRangeEndpoint,
    count: i32,
) -> Result<i32, LiveTextError> {
    if count == 0 {
        return Ok(0);
    }
    // SAFETY: COM call on a live interface pointer we own.
    unsafe { range.MoveEndpointByUnit(endpoint, TextUnit_Character, count) }
        .map_err(failed("MoveEndpointByUnit"))
}

/// The text of `range`, verbatim; `max_len` of -1 means all of it.
fn range_text(range: &IUIAutomationTextRange, max_len: i32) -> Option<String> {
    // SAFETY: COM call on a live interface pointer we own.
    let bstr = unsafe { range.GetText(max_len) }.ok()?;
    Some(String::from_utf16_lossy(&bstr))
}

/// The element's UIA runtime id joined with '.', e.g. "42.1234.4.17".
fn runtime_id(element: &IUIAutomationElement) -> Option<String> {
    // SAFETY: COM call; we own the returned SAFEARRAY and destroy it below.
    let array: *mut SAFEARRAY = unsafe { element.GetRuntimeId() }.ok()?;
    if array.is_null() {
        return None;
    }
    let id = safearray_i32_joined(array);
    // SAFETY: `array` came from GetRuntimeId and is destroyed exactly once.
    let _ = unsafe { SafeArrayDestroy(array) };
    id
}

fn safearray_i32_joined(array: *mut SAFEARRAY) -> Option<String> {
    // SAFETY: `array` is a valid SAFEARRAY; bounds are queried before access
    // and the data is only read between Access/UnaccessData. Runtime ids are
    // documented as one-dimensional VT_I4 arrays.
    unsafe {
        if SafeArrayGetDim(array) != 1 {
            return None;
        }
        let lower = SafeArrayGetLBound(array, 1).ok()?;
        let upper = SafeArrayGetUBound(array, 1).ok()?;
        let len = usize::try_from(i64::from(upper) - i64::from(lower) + 1).ok()?;
        if len == 0 {
            return None;
        }
        let mut data: *mut core::ffi::c_void = core::ptr::null_mut();
        SafeArrayAccessData(array, &mut data).ok()?;
        let joined = if data.is_null() {
            None
        } else {
            let values = core::slice::from_raw_parts(data as *const i32, len);
            Some(
                values
                    .iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join("."),
            )
        };
        let _ = SafeArrayUnaccessData(array);
        joined
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn to_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Keep the first `max_chars` UTF-16 units (on a char boundary); the flag is
/// true when nothing was cut.
fn truncate_chars(text: String, max_chars: usize) -> (String, bool) {
    let mut units = 0usize;
    for (i, c) in text.char_indices() {
        units += c.len_utf16();
        if units > max_chars {
            return (text[..i].to_string(), false);
        }
    }
    (text, true)
}

fn normalise_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}
