//! scrcpy client/server wire protocol, as implemented by scrcpy-server 5.0.1.
//!
//! Sources: `doc/develop.md`, `server/.../device/DesktopConnection.java` and
//! `server/.../device/Streamer.java` at tag v5.0.1. Everything is big-endian.

pub mod control;
pub mod device_msg;
pub mod video;

/// Size of the device name field sent once on the first socket
/// (`DesktopConnection.DEVICE_NAME_FIELD_LENGTH`).
pub const DEVICE_NAME_LEN: usize = 64;

/// Decodes the fixed 64-byte device name field: UTF-8, NUL-padded.
pub fn parse_device_name(field: &[u8; DEVICE_NAME_LEN]) -> String {
    let end = field
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(DEVICE_NAME_LEN);
    String::from_utf8_lossy(&field[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_name_stops_at_nul() {
        let mut field = [0u8; DEVICE_NAME_LEN];
        field[..7].copy_from_slice(b"RMX3771");
        assert_eq!(parse_device_name(&field), "RMX3771");
    }

    #[test]
    fn device_name_without_nul_uses_whole_field() {
        let field = [b'a'; DEVICE_NAME_LEN];
        assert_eq!(parse_device_name(&field), "a".repeat(DEVICE_NAME_LEN));
    }

    #[test]
    fn device_name_is_utf8() {
        let mut field = [0u8; DEVICE_NAME_LEN];
        let name = "réalme ⚡";
        field[..name.len()].copy_from_slice(name.as_bytes());
        assert_eq!(parse_device_name(&field), name);
    }
}
