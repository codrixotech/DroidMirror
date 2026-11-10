//! Client → device control messages.
//!
//! Byte layouts follow `ControlMessageReader.java` (server) and
//! `app/src/control_msg.c` (client) at scrcpy v5.0.1; the unit tests below
//! reuse the vectors from `app/tests/test_control_msg_serialize.c`.

/// `ControlMessage.TYPE_*` in the v5.0.1 server.
mod msg_type {
    pub const INJECT_KEYCODE: u8 = 0;
    pub const INJECT_TEXT: u8 = 1;
    pub const INJECT_TOUCH_EVENT: u8 = 2;
    pub const INJECT_SCROLL_EVENT: u8 = 3;
    pub const BACK_OR_SCREEN_ON: u8 = 4;
    pub const GET_CLIPBOARD: u8 = 8;
    pub const SET_CLIPBOARD: u8 = 9;
    pub const RESET_VIDEO: u8 = 17;
    pub const SCAN_FILE: u8 = 22;
}

/// `SC_CONTROL_MSG_INJECT_TEXT_MAX_LENGTH`.
pub const INJECT_TEXT_MAX_LEN: usize = 300;
/// `ControlMessageReader.CLIPBOARD_TEXT_MAX_LENGTH` (256 KiB message minus header).
pub const CLIPBOARD_TEXT_MAX_LEN: usize = (1 << 18) - 14;
/// `SC_CONTROL_MSG_SCAN_FILE_PATH_MAX_LENGTH`.
pub const SCAN_FILE_PATH_MAX_LEN: usize = 256;

/// `SC_POINTER_ID_GENERIC_FINGER`: injected as a real touchscreen finger.
pub const POINTER_ID_GENERIC_FINGER: u64 = -2i64 as u64;

/// Android `KeyEvent.ACTION_*` / `MotionEvent.ACTION_*` values we use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum KeyAction {
    Down = 0,
    Up = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TouchAction {
    Down = 0,
    Up = 1,
    Move = 2,
}

/// `GET_CLIPBOARD` copy key: which key to press before reading
/// (`SC_COPY_KEY_COPY`; 0 = none and 2 = cut are unused here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CopyKey {
    Copy = 1,
}

/// A point in video-frame pixels, plus the frame size it refers to. The
/// server ignores events whose size differs from the current video size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlMsg {
    InjectKeycode {
        action: KeyAction,
        keycode: u32,
        repeat: u32,
        meta_state: u32,
    },
    InjectText(String),
    InjectTouch {
        action: TouchAction,
        pointer_id: u64,
        position: Position,
        pressure: f32,
        action_button: u32,
        buttons: u32,
    },
    /// Scroll amounts in wheel "ticks", clamped to [-16, 16].
    InjectScroll {
        position: Position,
        hscroll: f32,
        vscroll: f32,
        buttons: u32,
    },
    BackOrScreenOn(KeyAction),
    GetClipboard(CopyKey),
    SetClipboard {
        sequence: u64,
        paste: bool,
        text: String,
    },
    ResetVideo,
    /// Ask the media scanner to index a file just pushed to the device.
    ScanFile(String),
}

impl ControlMsg {
    pub fn serialize(&self, out: &mut Vec<u8>) {
        match self {
            Self::InjectKeycode {
                action,
                keycode,
                repeat,
                meta_state,
            } => {
                out.push(msg_type::INJECT_KEYCODE);
                out.push(*action as u8);
                out.extend_from_slice(&keycode.to_be_bytes());
                out.extend_from_slice(&repeat.to_be_bytes());
                out.extend_from_slice(&meta_state.to_be_bytes());
            }
            Self::InjectText(text) => {
                out.push(msg_type::INJECT_TEXT);
                write_string(out, text, INJECT_TEXT_MAX_LEN);
            }
            Self::InjectTouch {
                action,
                pointer_id,
                position,
                pressure,
                action_button,
                buttons,
            } => {
                out.push(msg_type::INJECT_TOUCH_EVENT);
                out.push(*action as u8);
                out.extend_from_slice(&pointer_id.to_be_bytes());
                write_position(out, position);
                out.extend_from_slice(&float_to_u16fp(*pressure).to_be_bytes());
                out.extend_from_slice(&action_button.to_be_bytes());
                out.extend_from_slice(&buttons.to_be_bytes());
            }
            Self::InjectScroll {
                position,
                hscroll,
                vscroll,
                buttons,
            } => {
                out.push(msg_type::INJECT_SCROLL_EVENT);
                write_position(out, position);
                // The server decodes [-1, 1] and multiplies by 16.
                out.extend_from_slice(&float_to_i16fp(hscroll / 16.0).to_be_bytes());
                out.extend_from_slice(&float_to_i16fp(vscroll / 16.0).to_be_bytes());
                out.extend_from_slice(&buttons.to_be_bytes());
            }
            Self::BackOrScreenOn(action) => {
                out.push(msg_type::BACK_OR_SCREEN_ON);
                out.push(*action as u8);
            }
            Self::GetClipboard(key) => {
                out.push(msg_type::GET_CLIPBOARD);
                out.push(*key as u8);
            }
            Self::SetClipboard {
                sequence,
                paste,
                text,
            } => {
                out.push(msg_type::SET_CLIPBOARD);
                out.extend_from_slice(&sequence.to_be_bytes());
                out.push(*paste as u8);
                write_string(out, text, CLIPBOARD_TEXT_MAX_LEN);
            }
            Self::ResetVideo => out.push(msg_type::RESET_VIDEO),
            Self::ScanFile(path) => {
                out.push(msg_type::SCAN_FILE);
                write_string(out, path, SCAN_FILE_PATH_MAX_LEN);
            }
        }
    }
}

fn write_position(out: &mut Vec<u8>, p: &Position) {
    out.extend_from_slice(&p.x.to_be_bytes());
    out.extend_from_slice(&p.y.to_be_bytes());
    out.extend_from_slice(&p.width.to_be_bytes());
    out.extend_from_slice(&p.height.to_be_bytes());
}

/// u32 length + UTF-8 bytes, truncated on a char boundary to `max` bytes.
fn write_string(out: &mut Vec<u8>, s: &str, max: usize) {
    let s = truncate_utf8(s, max);
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

pub fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// `sc_float_to_u16fp`: [0, 1] → u16 fixed point, 1.0 saturating to 0xffff.
fn float_to_u16fp(f: f32) -> u16 {
    let u = (f.clamp(0.0, 1.0) * 65536.0) as u32;
    u.min(0xffff) as u16
}

/// `sc_float_to_i16fp`: [-1, 1] → i16 fixed point, 1.0 saturating to 0x7fff.
fn float_to_i16fp(f: f32) -> i16 {
    let i = (f.clamp(-1.0, 1.0) * 32768.0) as i32;
    i.clamp(-0x8000, 0x7fff) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(msg: &ControlMsg) -> Vec<u8> {
        let mut v = Vec::new();
        msg.serialize(&mut v);
        v
    }

    #[test]
    fn inject_keycode() {
        let msg = ControlMsg::InjectKeycode {
            action: KeyAction::Up,
            keycode: 66, // AKEYCODE_ENTER
            repeat: 5,
            meta_state: 0x41, // AMETA_SHIFT_ON | AMETA_SHIFT_LEFT_ON
        };
        assert_eq!(
            bytes(&msg),
            [0, 0x01, 0, 0, 0, 0x42, 0, 0, 0, 0x05, 0, 0, 0, 0x41]
        );
    }

    #[test]
    fn inject_text() {
        let mut expected = vec![1, 0, 0, 0, 0x0d];
        expected.extend_from_slice(b"hello, world!");
        assert_eq!(
            bytes(&ControlMsg::InjectText("hello, world!".into())),
            expected
        );
    }

    #[test]
    fn inject_text_is_truncated_on_char_boundary() {
        let text = "é".repeat(200); // 400 bytes
        let b = bytes(&ControlMsg::InjectText(text));
        let len = u32::from_be_bytes(b[1..5].try_into().unwrap()) as usize;
        assert_eq!(len, 300);
        assert!(std::str::from_utf8(&b[5..]).is_ok());
    }

    #[test]
    fn inject_touch_event() {
        let msg = ControlMsg::InjectTouch {
            action: TouchAction::Down,
            pointer_id: 0x1234_5678_8765_4321,
            position: Position {
                x: 100,
                y: 200,
                width: 1080,
                height: 1920,
            },
            pressure: 1.0,
            action_button: 1,
            buttons: 1,
        };
        assert_eq!(
            bytes(&msg),
            [
                2, 0x00, 0x12, 0x34, 0x56, 0x78, 0x87, 0x65, 0x43, 0x21, 0, 0, 0, 0x64, 0, 0, 0,
                0xc8, 0x04, 0x38, 0x07, 0x80, 0xff, 0xff, 0, 0, 0, 1, 0, 0, 0, 1,
            ]
        );
    }

    #[test]
    fn inject_scroll_event() {
        let msg = ControlMsg::InjectScroll {
            position: Position {
                x: 260,
                y: 1026,
                width: 1080,
                height: 1920,
            },
            hscroll: 16.0,
            vscroll: -16.0,
            buttons: 1,
        };
        assert_eq!(
            bytes(&msg),
            [
                3, 0, 0, 0x01, 0x04, 0, 0, 0x04, 0x02, 0x04, 0x38, 0x07, 0x80, 0x7f, 0xff, 0x80,
                0x00, 0, 0, 0, 1,
            ]
        );
    }

    #[test]
    fn back_or_screen_on() {
        assert_eq!(bytes(&ControlMsg::BackOrScreenOn(KeyAction::Up)), [4, 0x01]);
    }

    #[test]
    fn get_clipboard() {
        assert_eq!(bytes(&ControlMsg::GetClipboard(CopyKey::Copy)), [8, 1]);
    }

    #[test]
    fn set_clipboard() {
        let msg = ControlMsg::SetClipboard {
            sequence: 0x0102_0304_0506_0708,
            paste: true,
            text: "hello, world!".into(),
        };
        let mut expected = vec![9, 1, 2, 3, 4, 5, 6, 7, 8, 1, 0, 0, 0, 0x0d];
        expected.extend_from_slice(b"hello, world!");
        assert_eq!(bytes(&msg), expected);
    }

    #[test]
    fn reset_video() {
        assert_eq!(bytes(&ControlMsg::ResetVideo), [17]);
    }

    #[test]
    fn scan_file() {
        let mut expected = vec![22, 0, 0, 0, 5];
        expected.extend_from_slice(b"/a/b1");
        assert_eq!(bytes(&ControlMsg::ScanFile("/a/b1".into())), expected);
    }

    #[test]
    fn fixed_point_edges() {
        assert_eq!(float_to_u16fp(0.0), 0);
        assert_eq!(float_to_u16fp(0.5), 0x8000);
        assert_eq!(float_to_u16fp(1.0), 0xffff);
        assert_eq!(float_to_i16fp(1.0), 0x7fff);
        assert_eq!(float_to_i16fp(-1.0), -0x8000);
        assert_eq!(float_to_i16fp(0.0), 0);
    }

    #[test]
    fn generic_finger_id() {
        assert_eq!(POINTER_ID_GENERIC_FINGER, 0xffff_ffff_ffff_fffe);
    }
}
