//! Device → client messages on the control socket
//! (`DeviceMessageWriter.java` at scrcpy v5.0.1).

use std::io::{self, Read};

const TYPE_CLIPBOARD: u8 = 0;
const TYPE_ACK_CLIPBOARD: u8 = 1;
const TYPE_UHID_OUTPUT: u8 = 2;

/// `DeviceMessageWriter.CLIPBOARD_TEXT_MAX_LENGTH`.
const CLIPBOARD_TEXT_MAX_LEN: usize = (1 << 18) - 5;

#[derive(Debug, PartialEq, Eq)]
pub enum DeviceMsg {
    /// The device clipboard changed (or answered `GET_CLIPBOARD`).
    Clipboard(String),
    AckClipboard(u64),
    /// HID output report; unused (we don't create UHID devices).
    UhidOutput,
}

/// Reads one message; `Ok(None)` on a clean end of stream.
pub fn read(r: &mut impl Read) -> io::Result<Option<DeviceMsg>> {
    let mut ty = [0u8; 1];
    match r.read_exact(&mut ty) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    match ty[0] {
        TYPE_CLIPBOARD => {
            let len = read_u32(r)? as usize;
            if len > CLIPBOARD_TEXT_MAX_LEN {
                return Err(invalid(format!("clipboard length {len} too large")));
            }
            let mut buf = vec![0u8; len];
            r.read_exact(&mut buf)?;
            Ok(Some(DeviceMsg::Clipboard(
                String::from_utf8_lossy(&buf).into_owned(),
            )))
        }
        TYPE_ACK_CLIPBOARD => {
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            Ok(Some(DeviceMsg::AckClipboard(u64::from_be_bytes(b))))
        }
        TYPE_UHID_OUTPUT => {
            let mut head = [0u8; 4]; // id: u16, size: u16
            r.read_exact(&mut head)?;
            let size = u16::from_be_bytes([head[2], head[3]]) as u64;
            io::copy(&mut r.take(size), &mut io::sink())?;
            Ok(Some(DeviceMsg::UhidOutput))
        }
        other => Err(invalid(format!("unknown device message type {other}"))),
    }
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_messages_in_sequence() {
        // Same layouts as app/tests/test_device_msg_deserialize.c.
        let mut wire = vec![0, 0, 0, 0, 3];
        wire.extend_from_slice(b"ABC");
        wire.extend_from_slice(&[1, 1, 2, 3, 4, 5, 6, 7, 8]);
        wire.extend_from_slice(&[2, 0, 1, 0, 2, 0xaa, 0xbb]);
        let mut r = &wire[..];
        assert_eq!(
            read(&mut r).unwrap(),
            Some(DeviceMsg::Clipboard("ABC".into()))
        );
        assert_eq!(
            read(&mut r).unwrap(),
            Some(DeviceMsg::AckClipboard(0x0102_0304_0506_0708))
        );
        assert_eq!(read(&mut r).unwrap(), Some(DeviceMsg::UhidOutput));
        assert_eq!(read(&mut r).unwrap(), None);
    }

    #[test]
    fn rejects_unknown_type() {
        assert!(read(&mut &[9u8][..]).is_err());
    }

    #[test]
    fn truncated_clipboard_is_an_error() {
        assert!(read(&mut &[0u8, 0, 0, 0, 5, b'a'][..]).is_err());
    }
}
