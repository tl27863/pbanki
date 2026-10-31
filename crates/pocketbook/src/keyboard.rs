use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};

use inkview::bindings;

/// Capacity of the text buffer handed to InkView, including the trailing NUL.
const BUFFER_LEN: usize = 512;

/// Layout of the native keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardKind {
    Text,
    Url,
    Password,
}

impl KeyboardKind {
    fn flags(self) -> c_int {
        (match self {
            KeyboardKind::Text => bindings::KBD_NORMAL,
            KeyboardKind::Url => bindings::KBD_URL,
            KeyboardKind::Password => bindings::KBD_PASSWORD,
        }) as c_int
    }
}

/// Called once when the keyboard closes: `Some(text)` when the user
/// confirmed, `None` when the keyboard was cancelled.
pub type KeyboardDone = Box<dyn FnOnce(Option<String>) + Send>;

/// Owns everything InkView may read after `open` returns, until the keyboard
/// closes.
struct KeyboardContext {
    on_done: KeyboardDone,
    buffer: Box<[c_char]>,
    title: CString,
}

/// Opens the InkView keyboard. Returns immediately; `on_done` runs on the
/// thread that services InkView's event loop when the keyboard closes.
pub fn open(
    iv: &'static bindings::Inkview,
    title: &str,
    initial: &str,
    kind: KeyboardKind,
    on_done: KeyboardDone,
) {
    let mut buffer = vec![0 as c_char; BUFFER_LEN].into_boxed_slice();
    let initial = truncate_to_boundary(initial, BUFFER_LEN - 1);
    for (slot, byte) in buffer.iter_mut().zip(initial.as_bytes()) {
        *slot = *byte as c_char;
    }

    let title = CString::new(title.replace('\0', "")).unwrap_or_default();
    let context = Box::into_raw(Box::new(KeyboardContext {
        on_done,
        buffer,
        title,
    }));

    // SAFETY: the title and buffer pointers point into the heap-owned context,
    // which stays alive until `keyboard_closed` takes it back. The buffer is
    // NUL-terminated, and `maxlen` leaves room for the terminator.
    unsafe {
        iv.OpenKeyboardEx(
            (*context).title.as_ptr(),
            (*context).buffer.as_mut_ptr(),
            (BUFFER_LEN - 1) as c_int,
            kind.flags(),
            Some(keyboard_closed),
            context as *mut c_void,
        );
    }
}

/// InkView's keyboard close handler.
///
/// # Safety
///
/// - `data` is null or the pointer `open` passed to `OpenKeyboardEx`, i.e. a
///   `Box<KeyboardContext>` leaked with `Box::into_raw`.
/// - InkView calls the handler at most once per `OpenKeyboardEx`; a second
///   call with the same `data` would free the context twice.
/// - `text` is null (keyboard cancelled) or a NUL-terminated string that stays
///   valid for the duration of the call. It usually points into the context's
///   buffer, which is only freed after it has been copied.
unsafe extern "C" fn keyboard_closed(text: *mut c_char, data: *mut c_void) {
    if data.is_null() {
        return;
    }

    // SAFETY: `data` is the context leaked by `open`, reclaimed exactly once
    // here (see the function's safety contract).
    let context = unsafe { Box::from_raw(data as *mut KeyboardContext) };
    let value = (!text.is_null()).then(|| {
        // SAFETY: non-null `text` is NUL-terminated and valid for this call;
        // it is copied into a `String` while `context` (and so its buffer) is
        // still alive.
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    });
    (context.on_done)(value);
}

/// Returns the longest prefix of `text` that fits in `max_bytes` without
/// splitting a character.
fn truncate_to_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::truncate_to_boundary;

    #[test]
    fn keeps_short_text() {
        assert_eq!(truncate_to_boundary("abc", 10), "abc");
    }

    #[test]
    fn truncates_ascii_at_limit() {
        assert_eq!(truncate_to_boundary("abcdef", 4), "abcd");
    }

    #[test]
    fn does_not_split_multibyte_characters() {
        // "é" is two bytes; a limit of 2 would land inside it.
        assert_eq!(truncate_to_boundary("aé", 2), "a");
        assert_eq!(truncate_to_boundary("日本", 4), "日");
    }
}
