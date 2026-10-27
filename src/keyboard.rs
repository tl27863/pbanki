use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::mpsc::Sender;

use inkview::bindings;

pub const FIELD_ENDPOINT: i32 = 0;
pub const FIELD_USERNAME: i32 = 1;
pub const FIELD_PASSWORD: i32 = 2;

struct KeyboardContext {
    field: i32,
    sender: Sender<(i32, String)>,
    buffer: Box<[c_char]>,
    _title: CString,
}

pub fn open(
    iv: &'static bindings::Inkview,
    field: i32,
    title: &str,
    initial: &str,
    flags: c_int,
    sender: Sender<(i32, String)>,
) {
    let mut buffer = vec![0 as c_char; 512];
    for (slot, byte) in buffer.iter_mut().zip(initial.as_bytes()) {
        *slot = *byte as c_char;
    }

    let title = CString::new(title).unwrap_or_default();
    let maxlen = buffer.len() as c_int;
    let context = Box::new(KeyboardContext {
        field,
        sender,
        buffer: buffer.into_boxed_slice(),
        _title: title.clone(),
    });
    let context = Box::into_raw(context);

    unsafe {
        iv.OpenKeyboardEx(
            title.as_ptr(),
            (*context).buffer.as_mut_ptr(),
            maxlen,
            flags,
            Some(keyboard_closed),
            context as *mut c_void,
        );
    }
}

unsafe extern "C" fn keyboard_closed(text: *mut c_char, data: *mut c_void) {
    if data.is_null() {
        return;
    }

    let context = unsafe { Box::from_raw(data as *mut KeyboardContext) };
    let value = if text.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    };
    let _ = context.sender.send((context.field, value));
}
