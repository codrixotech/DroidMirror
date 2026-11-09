//! Video socket: codec id, then session packets and media packets.
//!
//! ```text
//! codec id      u32                       (once, after the device name)
//! session pkt   u32 flags | u32 w | u32 h (MSB of flags = 1; bit 0 = client resized)
//! media pkt     u64 flags+pts | u32 len | len bytes
//!               bit 63 = 0, bit 62 = config, bit 61 = key frame, bits 0..61 = PTS
//! ```
//!
//! A codec id of 0 means the device disabled the stream, 1 means a
//! configuration error on the device (`Streamer.writeDisableStream`).

use std::io::{self, Read};

pub const CODEC_ID_H264: u32 = 0x6832_3634; // "h264"

pub const HEADER_LEN: usize = 12;

const FLAG_SESSION: u64 = 1 << 63;
const FLAG_CONFIG: u64 = 1 << 62;
const FLAG_KEY_FRAME: u64 = 1 << 61;
const PTS_MASK: u64 = FLAG_KEY_FRAME - 1;

/// Upper bound on a single packet. Real H.264 packets at our settings are
/// well under 1 MiB; anything larger means the stream is out of sync.
pub const MAX_PACKET_LEN: u32 = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    #[error("the device disabled the video stream")]
    Disabled,
    #[error("the device reported a configuration error (see the server log above)")]
    DeviceConfigError,
    #[error("unsupported video codec id {0:#010x} (expected h264)")]
    UnsupportedCodec(u32),
    #[error("invalid packet length {0} (stream out of sync)")]
    BadLength(u32),
    #[error("video stream ended in the middle of a packet")]
    Truncated,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Checks the codec id; DroidMirror only requests and accepts H.264.
pub fn parse_codec_id(id: u32) -> Result<(), VideoError> {
    match id {
        CODEC_ID_H264 => Ok(()),
        0 => Err(VideoError::Disabled),
        1 => Err(VideoError::DeviceConfigError),
        other => Err(VideoError::UnsupportedCodec(other)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Header {
    /// Start of a capture session (first frame, rotation, resize).
    Session {
        width: u32,
        height: u32,
        client_resized: bool,
    },
    /// A packet of `len` bytes follows. `pts` is `None` for config packets.
    Media {
        config: bool,
        key_frame: bool,
        pts: Option<u64>,
        len: u32,
    },
}

pub fn parse_header(b: &[u8; HEADER_LEN]) -> Result<Header, VideoError> {
    let first = u64::from_be_bytes(b[0..8].try_into().unwrap());
    let last = u32::from_be_bytes(b[8..12].try_into().unwrap());

    if first & FLAG_SESSION != 0 {
        let flags = (first >> 32) as u32;
        return Ok(Header::Session {
            width: first as u32,
            height: last,
            client_resized: flags & 1 != 0,
        });
    }

    if last == 0 || last > MAX_PACKET_LEN {
        return Err(VideoError::BadLength(last));
    }
    let config = first & FLAG_CONFIG != 0;
    Ok(Header::Media {
        config,
        key_frame: first & FLAG_KEY_FRAME != 0,
        pts: (!config).then_some(first & PTS_MASK),
        len: last,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct Packet {
    pub config: bool,
    pub key_frame: bool,
    pub pts: Option<u64>,
    /// Raw H.264 Annex B payload, as produced by `MediaCodec`.
    pub data: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Session { width: u32, height: u32 },
    Packet(Packet),
}

/// Reads the video socket after the device name.
pub struct VideoStream<R> {
    inner: R,
}

impl<R: Read> VideoStream<R> {
    pub fn new(inner: R) -> Self {
        Self { inner }
    }

    /// Reads and checks the codec id. Must be called once, first.
    pub fn read_codec(&mut self) -> Result<(), VideoError> {
        let mut b = [0u8; 4];
        self.inner.read_exact(&mut b)?;
        parse_codec_id(u32::from_be_bytes(b))
    }

    /// Next session or media packet; `Ok(None)` on a clean end of stream.
    pub fn next_event(&mut self) -> Result<Option<Event>, VideoError> {
        let mut h = [0u8; HEADER_LEN];
        if !read_exact_or_eof(&mut self.inner, &mut h)? {
            return Ok(None);
        }
        match parse_header(&h)? {
            Header::Session { width, height, .. } => Ok(Some(Event::Session { width, height })),
            Header::Media {
                config,
                key_frame,
                pts,
                len,
            } => {
                let mut data = vec![0u8; len as usize];
                self.inner
                    .read_exact(&mut data)
                    .map_err(|e| match e.kind() {
                        io::ErrorKind::UnexpectedEof => VideoError::Truncated,
                        _ => VideoError::Io(e),
                    })?;
                Ok(Some(Event::Packet(Packet {
                    config,
                    key_frame,
                    pts,
                    data,
                })))
            }
        }
    }
}

/// Fills `buf`, returning `false` if the stream ended before the first byte.
fn read_exact_or_eof(r: &mut impl Read, buf: &mut [u8]) -> Result<bool, VideoError> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(VideoError::Truncated),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(w: u32, h: u32, resized: bool) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&(0x8000_0000u32 | resized as u32).to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v
    }

    /// Mirrors `Streamer.writeFrameMeta`.
    fn media(config: bool, key: bool, pts: u64, data: &[u8]) -> Vec<u8> {
        let flags = if config {
            FLAG_CONFIG
        } else {
            pts | if key { FLAG_KEY_FRAME } else { 0 }
        };
        let mut v = Vec::new();
        v.extend_from_slice(&flags.to_be_bytes());
        v.extend_from_slice(&(data.len() as u32).to_be_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn codec_ids() {
        assert!(parse_codec_id(u32::from_be_bytes(*b"h264")).is_ok());
        assert!(matches!(parse_codec_id(0), Err(VideoError::Disabled)));
        assert!(matches!(
            parse_codec_id(1),
            Err(VideoError::DeviceConfigError)
        ));
        let h265 = u32::from_be_bytes(*b"h265");
        assert!(
            matches!(parse_codec_id(h265), Err(VideoError::UnsupportedCodec(id)) if id == h265)
        );
        assert_eq!(CODEC_ID_H264, 0x6832_3634);
    }

    #[test]
    fn session_header() {
        let b: [u8; 12] = session(1024, 472, false).try_into().unwrap();
        assert_eq!(
            parse_header(&b).unwrap(),
            Header::Session {
                width: 1024,
                height: 472,
                client_resized: false
            }
        );
        let b: [u8; 12] = session(472, 1024, true).try_into().unwrap();
        assert_eq!(
            parse_header(&b).unwrap(),
            Header::Session {
                width: 472,
                height: 1024,
                client_resized: true
            }
        );
    }

    #[test]
    fn config_header_has_no_pts() {
        let b: [u8; 12] = media(true, false, 0, &[0; 30])[..12].try_into().unwrap();
        assert_eq!(
            parse_header(&b).unwrap(),
            Header::Media {
                config: true,
                key_frame: false,
                pts: None,
                len: 30
            }
        );
    }

    #[test]
    fn key_frame_header_keeps_full_pts() {
        let pts = (1u64 << 61) - 1; // largest 61-bit PTS
        let b: [u8; 12] = media(false, true, pts, &[0; 5])[..12].try_into().unwrap();
        assert_eq!(
            parse_header(&b).unwrap(),
            Header::Media {
                config: false,
                key_frame: true,
                pts: Some(pts),
                len: 5
            }
        );
    }

    #[test]
    fn known_bytes() {
        // Delta frame, PTS 0x0102 µs, 3 bytes.
        let b = [0, 0, 0, 0, 0, 0, 1, 2, 0, 0, 0, 3];
        assert_eq!(
            parse_header(&b).unwrap(),
            Header::Media {
                config: false,
                key_frame: false,
                pts: Some(0x0102),
                len: 3
            }
        );
        // Key frame flag is bit 61 (0x20 in the first byte).
        let b = [0x20, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 1];
        assert!(matches!(
            parse_header(&b).unwrap(),
            Header::Media {
                key_frame: true,
                pts: Some(9),
                ..
            }
        ));
    }

    #[test]
    fn rejects_bad_lengths() {
        let mut b = [0u8; 12];
        assert!(matches!(parse_header(&b), Err(VideoError::BadLength(0))));
        b[8..].copy_from_slice(&(MAX_PACKET_LEN + 1).to_be_bytes());
        assert!(matches!(parse_header(&b), Err(VideoError::BadLength(_))));
    }

    #[test]
    fn reads_a_full_stream() {
        let mut wire = Vec::new();
        wire.extend_from_slice(b"h264");
        wire.extend(session(472, 1024, false));
        wire.extend(media(true, false, 0, b"\0\0\0\x01\x67sps"));
        wire.extend(media(false, true, 33_000, b"\0\0\0\x01\x65idr"));
        wire.extend(session(1024, 472, false)); // rotation
        wire.extend(media(false, false, 66_000, b"\0\0\0\x01\x41p"));

        // A reader that returns one byte at a time exercises partial reads.
        struct Trickle<'a>(&'a [u8]);
        impl Read for Trickle<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.0.is_empty() || buf.is_empty() {
                    return Ok(0);
                }
                buf[0] = self.0[0];
                self.0 = &self.0[1..];
                Ok(1)
            }
        }

        let mut s = VideoStream::new(Trickle(&wire));
        s.read_codec().unwrap();
        assert_eq!(
            s.next_event().unwrap(),
            Some(Event::Session {
                width: 472,
                height: 1024
            })
        );
        let Some(Event::Packet(p)) = s.next_event().unwrap() else {
            panic!()
        };
        assert!(p.config && p.pts.is_none());
        assert_eq!(p.data, b"\0\0\0\x01\x67sps");
        let Some(Event::Packet(p)) = s.next_event().unwrap() else {
            panic!()
        };
        assert!(p.key_frame && p.pts == Some(33_000));
        assert_eq!(
            s.next_event().unwrap(),
            Some(Event::Session {
                width: 1024,
                height: 472
            })
        );
        let Some(Event::Packet(p)) = s.next_event().unwrap() else {
            panic!()
        };
        assert!(!p.key_frame && !p.config);
        assert_eq!(s.next_event().unwrap(), None);
    }

    #[test]
    fn truncated_payload_is_an_error() {
        let mut wire = media(false, false, 1, b"abcdef");
        wire.truncate(wire.len() - 2);
        let mut s = VideoStream::new(&wire[..]);
        assert!(matches!(s.next_event(), Err(VideoError::Truncated)));
    }

    #[test]
    fn truncated_header_is_an_error() {
        let wire = [0u8; 5];
        let mut s = VideoStream::new(&wire[..]);
        assert!(matches!(s.next_event(), Err(VideoError::Truncated)));
    }
}
