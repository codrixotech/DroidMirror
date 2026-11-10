//! Platform-neutral input mapping: macOS virtual key codes and modifier
//! flags → Android key codes and meta state, view points → video pixels,
//! and window geometry. The AppKit glue in `macos/` only extracts raw
//! values from `NSEvent` and calls into here.

/// Android `KeyEvent.KEYCODE_*` values used directly by the app.
pub mod keycode {
    pub const HOME: u32 = 3;
    pub const BACK: u32 = 4;
    pub const APP_SWITCH: u32 = 187;
}

/// `NSEventModifierFlags` bits.
pub mod ns_flags {
    pub const CAPS_LOCK: u64 = 1 << 16;
    pub const SHIFT: u64 = 1 << 17;
    pub const CONTROL: u64 = 1 << 18;
    pub const OPTION: u64 = 1 << 19;
    pub const COMMAND: u64 = 1 << 20;
}

/// Android `KeyEvent.META_*` for the current modifier flags.
pub fn meta_state(flags: u64) -> u32 {
    use ns_flags::*;
    let mut m = 0;
    if flags & SHIFT != 0 {
        m |= 0x1 | 0x40; // META_SHIFT_ON | META_SHIFT_LEFT_ON
    }
    if flags & OPTION != 0 {
        m |= 0x2 | 0x10; // META_ALT_ON | META_ALT_LEFT_ON
    }
    if flags & CONTROL != 0 {
        m |= 0x1000 | 0x2000; // META_CTRL_ON | META_CTRL_LEFT_ON
    }
    if flags & COMMAND != 0 {
        m |= 0x10000 | 0x20000; // META_META_ON | META_META_LEFT_ON
    }
    if flags & CAPS_LOCK != 0 {
        m |= 0x100000; // META_CAPS_LOCK_ON
    }
    m
}

/// Keys that never produce text and are always sent as key codes.
/// (macOS `kVK_*` → Android `KEYCODE_*`.)
const SPECIAL_KEYS: &[(u16, u32)] = &[
    (0x24, 66),  // Return → ENTER
    (0x4c, 160), // keypad Enter → NUMPAD_ENTER
    (0x33, 67),  // Delete (backspace) → DEL
    (0x75, 112), // Forward delete → FORWARD_DEL
    (0x30, 61),  // Tab → TAB
    (0x35, 111), // Escape → ESCAPE
    (0x7b, 21),  // Left → DPAD_LEFT
    (0x7c, 22),  // Right → DPAD_RIGHT
    (0x7d, 20),  // Down → DPAD_DOWN
    (0x7e, 19),  // Up → DPAD_UP
    (0x73, 122), // Home → MOVE_HOME
    (0x77, 123), // End → MOVE_END
    (0x74, 92),  // Page up → PAGE_UP
    (0x79, 93),  // Page down → PAGE_DOWN
    (0x7a, 131), // F1
    (0x78, 132), // F2
    (0x63, 133), // F3
    (0x76, 134), // F4
    (0x60, 135), // F5
    (0x61, 136), // F6
    (0x62, 137), // F7
    (0x64, 138), // F8
    (0x65, 139), // F9
    (0x6d, 140), // F10
    (0x67, 141), // F11
    (0x6f, 142), // F12
];

/// Character keys, sent as key codes only with Control held (Ctrl+A,
/// Ctrl+C, ...); otherwise their text is injected.
const CHAR_KEYS: &[(u16, u32)] = &[
    (0x00, 29), // A
    (0x0b, 30), // B
    (0x08, 31), // C
    (0x02, 32), // D
    (0x0e, 33), // E
    (0x03, 34), // F
    (0x05, 35), // G
    (0x04, 36), // H
    (0x22, 37), // I
    (0x26, 38), // J
    (0x28, 39), // K
    (0x25, 40), // L
    (0x2e, 41), // M
    (0x2d, 42), // N
    (0x1f, 43), // O
    (0x23, 44), // P
    (0x0c, 45), // Q
    (0x0f, 46), // R
    (0x01, 47), // S
    (0x11, 48), // T
    (0x20, 49), // U
    (0x09, 50), // V
    (0x0d, 51), // W
    (0x07, 52), // X
    (0x10, 53), // Y
    (0x06, 54), // Z
    (0x1d, 7),  // 0
    (0x12, 8),  // 1
    (0x13, 9),  // 2
    (0x14, 10), // 3
    (0x15, 11), // 4
    (0x17, 12), // 5
    (0x16, 13), // 6
    (0x1a, 14), // 7
    (0x1c, 15), // 8
    (0x19, 16), // 9
    (0x31, 62), // Space
    (0x2b, 55), // , COMMA
    (0x2f, 56), // . PERIOD
    (0x32, 68), // ` GRAVE
    (0x1b, 69), // - MINUS
    (0x18, 70), // = EQUALS
    (0x21, 71), // [ LEFT_BRACKET
    (0x1e, 72), // ] RIGHT_BRACKET
    (0x2a, 73), // \ BACKSLASH
    (0x29, 74), // ; SEMICOLON
    (0x27, 75), // ' APOSTROPHE
    (0x2c, 76), // / SLASH
];

fn lookup(table: &[(u16, u32)], key: u16) -> Option<u32> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// What a key press should become on the device.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyInput {
    /// Send `KEYCODE` down now and up on key release.
    Keycode(u32),
    /// Inject this text (on key down only).
    Text(String),
    Ignore,
}

/// Mixed mode, like scrcpy's default: special keys and Ctrl+key are sent
/// as key codes, everything else as text (so any keyboard layout, and
/// Option-characters like "é", work).
pub fn map_key_down(mac_keycode: u16, flags: u64, characters: &str) -> KeyInput {
    if flags & ns_flags::COMMAND != 0 {
        return KeyInput::Ignore; // shortcuts are handled by the menu
    }
    if let Some(k) = lookup(SPECIAL_KEYS, mac_keycode) {
        return KeyInput::Keycode(k);
    }
    if flags & ns_flags::CONTROL != 0 {
        return lookup(CHAR_KEYS, mac_keycode).map_or(KeyInput::Ignore, KeyInput::Keycode);
    }
    let text: String = characters.chars().filter(|c| is_printable(*c)).collect();
    if text.is_empty() {
        KeyInput::Ignore
    } else {
        KeyInput::Text(text)
    }
}

/// Excludes control characters and AppKit's function-key private-use
/// range (U+F700–U+F8FF).
fn is_printable(c: char) -> bool {
    !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(&c)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Largest rect with the content's aspect ratio centered in `container`
/// (what `AVLayerVideoGravityResizeAspect` draws).
pub fn fit_rect(container: Rect, content_w: u32, content_h: u32) -> Rect {
    if content_w == 0 || content_h == 0 {
        return container;
    }
    let scale = (container.w / content_w as f64).min(container.h / content_h as f64);
    let w = content_w as f64 * scale;
    let h = content_h as f64 * scale;
    Rect {
        x: container.x + (container.w - w) / 2.0,
        y: container.y + (container.h - h) / 2.0,
        w,
        h,
    }
}

/// Maps a point (top-left origin) inside the drawn video to frame pixels.
/// With `clamp`, points outside are pinned to the edge (for drags that
/// leave the video); otherwise they return `None`.
pub fn map_point(
    video: Rect,
    frame_w: u32,
    frame_h: u32,
    x: f64,
    y: f64,
    clamp: bool,
) -> Option<(i32, i32)> {
    if video.w <= 0.0 || video.h <= 0.0 || frame_w == 0 || frame_h == 0 {
        return None;
    }
    if !clamp && !video.contains(x, y) {
        return None;
    }
    let fx = ((x - video.x) / video.w * frame_w as f64).floor();
    let fy = ((y - video.y) / video.h * frame_h as f64).floor();
    Some((
        fx.clamp(0.0, frame_w as f64 - 1.0) as i32,
        fy.clamp(0.0, frame_h as f64 - 1.0) as i32,
    ))
}

/// Converts a scroll-wheel delta to Android scroll "ticks". Trackpads
/// report precise pixel deltas, mice report lines.
pub fn scroll_ticks(delta: f64, precise: bool) -> f32 {
    let ticks = if precise { delta / 10.0 } else { delta };
    ticks.clamp(-16.0, 16.0) as f32
}

/// Window content size for a video size: `bezel` points around the video,
/// height at most `max_h`, width at most `max_w`.
pub fn window_size(video_w: u32, video_h: u32, max_w: f64, max_h: f64, bezel: f64) -> (f64, f64) {
    let (vw, vh) = (video_w.max(1) as f64, video_h.max(1) as f64);
    let scale = ((max_w - 2.0 * bezel) / vw).min((max_h - 2.0 * bezel) / vh);
    (
        (vw * scale + 2.0 * bezel).round(),
        (vh * scale + 2.0 * bezel).round(),
    )
}

/// Adjusts a proposed window size during a live resize so the video area
/// keeps its aspect ratio (the bezel stays constant).
pub fn constrain_resize(
    proposed_w: f64,
    proposed_h: f64,
    video_w: u32,
    video_h: u32,
    bezel: f64,
) -> (f64, f64) {
    let (vw, vh) = (video_w.max(1) as f64, video_h.max(1) as f64);
    let min = 120.0;
    let inner_w = (proposed_w - 2.0 * bezel).max(min);
    let inner_h = (proposed_h - 2.0 * bezel).max(min);
    // Follow whichever dimension changed more relative to the video.
    let scale = (inner_w / vw).max(inner_h / vh);
    (
        (vw * scale + 2.0 * bezel).round(),
        (vh * scale + 2.0 * bezel).round(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_to_meta_state() {
        assert_eq!(meta_state(0), 0);
        assert_eq!(meta_state(ns_flags::SHIFT), 0x41);
        assert_eq!(meta_state(ns_flags::CONTROL | ns_flags::OPTION), 0x3012);
        assert_eq!(meta_state(ns_flags::CAPS_LOCK), 0x100000);
    }

    #[test]
    fn special_keys_are_keycodes() {
        assert_eq!(map_key_down(0x24, 0, "\r"), KeyInput::Keycode(66));
        assert_eq!(map_key_down(0x33, 0, "\u{7f}"), KeyInput::Keycode(67));
        assert_eq!(map_key_down(0x7e, 0, "\u{f700}"), KeyInput::Keycode(19));
        assert_eq!(
            map_key_down(0x35, ns_flags::SHIFT, "\u{1b}"),
            KeyInput::Keycode(111)
        );
    }

    #[test]
    fn characters_are_text() {
        assert_eq!(map_key_down(0x00, 0, "a"), KeyInput::Text("a".into()));
        assert_eq!(
            map_key_down(0x00, ns_flags::SHIFT, "A"),
            KeyInput::Text("A".into())
        );
        assert_eq!(
            map_key_down(0x0e, ns_flags::OPTION, "é"),
            KeyInput::Text("é".into())
        );
        assert_eq!(map_key_down(0x31, 0, " "), KeyInput::Text(" ".into()));
    }

    #[test]
    fn control_combos_are_keycodes() {
        assert_eq!(
            map_key_down(0x00, ns_flags::CONTROL, "\u{1}"),
            KeyInput::Keycode(29)
        );
        assert_eq!(
            map_key_down(0x08, ns_flags::CONTROL, "\u{3}"),
            KeyInput::Keycode(31)
        );
    }

    #[test]
    fn command_and_dead_keys_are_ignored() {
        assert_eq!(map_key_down(0x0b, ns_flags::COMMAND, "b"), KeyInput::Ignore);
        assert_eq!(map_key_down(0x0e, ns_flags::OPTION, ""), KeyInput::Ignore);
        assert_eq!(map_key_down(0x60, 0, "\u{f708}"), KeyInput::Keycode(135));
        assert_eq!(map_key_down(0x47, 0, "\u{f739}"), KeyInput::Ignore); // Clear, unmapped
    }

    #[test]
    fn tables_have_unique_mac_keys() {
        let mut keys: Vec<u16> = SPECIAL_KEYS
            .iter()
            .chain(CHAR_KEYS)
            .map(|(k, _)| *k)
            .collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n);
    }

    #[test]
    fn fit_rect_letterboxes() {
        let c = Rect {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 200.0,
        };
        assert_eq!(
            fit_rect(c, 100, 200),
            Rect {
                x: 50.0,
                y: 0.0,
                w: 100.0,
                h: 200.0
            }
        );
        assert_eq!(
            fit_rect(c, 200, 100),
            Rect {
                x: 0.0,
                y: 50.0,
                w: 200.0,
                h: 100.0
            }
        );
    }

    #[test]
    fn maps_points_to_frame_pixels() {
        let v = Rect {
            x: 10.0,
            y: 10.0,
            w: 236.0,
            h: 512.0,
        };
        assert_eq!(map_point(v, 472, 1024, 10.0, 10.0, false), Some((0, 0)));
        assert_eq!(
            map_point(v, 472, 1024, 128.0, 266.0, false),
            Some((236, 512))
        );
        assert_eq!(
            map_point(v, 472, 1024, 245.9, 521.9, false),
            Some((471, 1023))
        );
        assert_eq!(map_point(v, 472, 1024, 5.0, 100.0, false), None);
        assert_eq!(map_point(v, 472, 1024, 5.0, 600.0, true), Some((0, 1023)));
        assert_eq!(map_point(v, 0, 0, 20.0, 20.0, false), None);
    }

    #[test]
    fn scroll_scaling() {
        assert_eq!(scroll_ticks(1.0, false), 1.0);
        assert_eq!(scroll_ticks(25.0, true), 2.5);
        assert_eq!(scroll_ticks(-500.0, true), -16.0);
    }

    #[test]
    fn window_sizes_keep_video_aspect() {
        let (w, h) = window_size(472, 1024, 1000.0, 800.0, 10.0);
        assert_eq!(h, 800.0);
        assert!((((w - 20.0) / (h - 20.0)) - 472.0 / 1024.0).abs() < 0.01);

        let (w, h) = window_size(1024, 472, 600.0, 800.0, 10.0);
        assert_eq!(w, 600.0);
        assert!(h < 800.0);

        let (w, h) = constrain_resize(400.0, 500.0, 472, 1024, 10.0);
        assert!((((w - 20.0) / (h - 20.0)) - 472.0 / 1024.0).abs() < 0.01);
    }
}
