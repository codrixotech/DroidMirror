//! System clipboard (`NSPasteboard.generalPasteboard`), plain text only.

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

pub fn change_count() -> isize {
    NSPasteboard::generalPasteboard().changeCount()
}

pub fn get_text() -> Option<String> {
    // SAFETY: reading an immutable AppKit constant.
    let ty = unsafe { NSPasteboardTypeString };
    NSPasteboard::generalPasteboard()
        .stringForType(ty)
        .map(|s| s.to_string())
}

/// Sets the clipboard and returns the new change count.
pub fn set_text(text: &str) -> isize {
    let pb = NSPasteboard::generalPasteboard();
    // SAFETY: reading an immutable AppKit constant.
    let ty = unsafe { NSPasteboardTypeString };
    pb.clearContents();
    pb.setString_forType(&NSString::from_str(text), ty);
    pb.changeCount()
}
