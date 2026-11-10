//! H.264 Annex B helpers: split NAL units, pull out SPS/PPS, and convert
//! the rest to AVCC (4-byte big-endian length prefixes), which is what a
//! `CMSampleBuffer` with an H.264 `CMVideoFormatDescription` expects.

pub const NAL_SLICE: u8 = 1;
pub const NAL_IDR: u8 = 5;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;
pub const NAL_AUD: u8 = 9;

/// AVCC length prefix size, passed to CoreMedia as `nal_unit_header_length`.
pub const AVCC_LENGTH_SIZE: usize = 4;

pub fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |b| b & 0x1f)
}

/// Iterates over the NAL units of an Annex B buffer (3- or 4-byte start
/// codes), without the start codes. Bytes before the first start code
/// are ignored.
pub fn nal_units(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = match find_start_code(data) {
        Some((pos, len)) => &data[pos + len..],
        None => &data[data.len()..],
    };
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let (nal, next) = match find_start_code(rest) {
            Some((pos, len)) => (&rest[..pos], &rest[pos + len..]),
            None => (rest, &rest[rest.len()..]),
        };
        rest = next;
        // Trailing zero bytes belong to the next 4-byte start code.
        let end = nal.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        Some(&nal[..end])
    })
    .filter(|nal| !nal.is_empty())
}

/// Finds the next `00 00 01` and returns (position, start code length),
/// counting a preceding zero as part of a 4-byte start code.
fn find_start_code(data: &[u8]) -> Option<(usize, usize)> {
    let i = data.windows(3).position(|w| w == [0, 0, 1])?;
    if i > 0 && data[i - 1] == 0 {
        Some((i - 1, 4))
    } else {
        Some((i, 3))
    }
}

/// SPS and PPS found in a buffer (the last of each wins).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParamSets {
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
}

impl ParamSets {
    pub fn extract(data: &[u8]) -> Self {
        let mut p = Self::default();
        for nal in nal_units(data) {
            match nal_type(nal) {
                NAL_SPS => p.sps = Some(nal.to_vec()),
                NAL_PPS => p.pps = Some(nal.to_vec()),
                _ => {}
            }
        }
        p
    }

    pub fn is_empty(&self) -> bool {
        self.sps.is_none() && self.pps.is_none()
    }
}

/// Converts Annex B to AVCC into `out` (cleared first). SPS, PPS and AUD
/// units are dropped: they live in the format description, not the sample.
/// Returns `true` if any slice data was written.
pub fn annexb_to_avcc(data: &[u8], out: &mut Vec<u8>) -> bool {
    out.clear();
    let mut has_slice = false;
    for nal in nal_units(data) {
        let ty = nal_type(nal);
        if matches!(ty, NAL_SPS | NAL_PPS | NAL_AUD) {
            continue;
        }
        has_slice |= (NAL_SLICE..=NAL_IDR).contains(&ty);
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    has_slice
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: &[u8] = &[0x67, 0x42, 0xc0, 0x1f, 0xda];
    const PPS: &[u8] = &[0x68, 0xce, 0x3c, 0x80];
    const IDR: &[u8] = &[0x65, 0x88, 0x84, 0x00, 0x21];
    const P: &[u8] = &[0x41, 0x9a, 0x02];

    fn annexb(nals: &[&[u8]], four: bool) -> Vec<u8> {
        let mut v = Vec::new();
        for n in nals {
            v.extend_from_slice(if four { &[0, 0, 0, 1] } else { &[0, 0, 1] });
            v.extend_from_slice(n);
        }
        v
    }

    fn avcc(nals: &[&[u8]]) -> Vec<u8> {
        let mut v = Vec::new();
        for n in nals {
            v.extend_from_slice(&(n.len() as u32).to_be_bytes());
            v.extend_from_slice(n);
        }
        v
    }

    #[test]
    fn splits_four_byte_start_codes() {
        let data = annexb(&[SPS, PPS, IDR], true);
        assert_eq!(nal_units(&data).collect::<Vec<_>>(), [SPS, PPS, IDR]);
    }

    #[test]
    fn splits_three_byte_and_mixed_start_codes() {
        let data = annexb(&[SPS, PPS], false);
        assert_eq!(nal_units(&data).collect::<Vec<_>>(), [SPS, PPS]);

        let mut mixed = annexb(&[SPS], true);
        mixed.extend(annexb(&[P], false));
        mixed.extend(annexb(&[IDR], true));
        assert_eq!(nal_units(&mixed).collect::<Vec<_>>(), [SPS, P, IDR]);
    }

    #[test]
    fn ignores_leading_garbage_and_empty_input() {
        let mut data = vec![0xff, 0xee];
        data.extend(annexb(&[P], true));
        assert_eq!(nal_units(&data).collect::<Vec<_>>(), [P]);
        assert_eq!(nal_units(&[]).count(), 0);
        assert_eq!(nal_units(&[1, 2, 3]).count(), 0);
        assert_eq!(nal_units(&[0, 0, 0, 1]).count(), 0);
    }

    #[test]
    fn keeps_emulation_prevention_bytes() {
        // 00 00 03 is not a start code and must be passed through intact.
        let nal: &[u8] = &[0x41, 0x00, 0x00, 0x03, 0x01, 0x7f];
        let data = annexb(&[nal], true);
        assert_eq!(nal_units(&data).collect::<Vec<_>>(), [nal]);
    }

    #[test]
    fn nal_types() {
        assert_eq!(nal_type(SPS), NAL_SPS);
        assert_eq!(nal_type(PPS), NAL_PPS);
        assert_eq!(nal_type(IDR), NAL_IDR);
        assert_eq!(nal_type(P), NAL_SLICE);
        assert_eq!(nal_type(&[]), 0);
    }

    #[test]
    fn extracts_parameter_sets() {
        let p = ParamSets::extract(&annexb(&[SPS, PPS], true));
        assert_eq!(p.sps.as_deref(), Some(SPS));
        assert_eq!(p.pps.as_deref(), Some(PPS));
        assert!(ParamSets::extract(&annexb(&[P], true)).is_empty());
    }

    #[test]
    fn converts_frame_to_avcc() {
        let mut out = vec![0xaa; 3]; // must be cleared
        assert!(annexb_to_avcc(&annexb(&[P], true), &mut out));
        assert_eq!(out, avcc(&[P]));
    }

    #[test]
    fn strips_parameter_sets_and_aud_from_samples() {
        let aud: &[u8] = &[0x09, 0xf0];
        let sei: &[u8] = &[0x06, 0x05, 0x01];
        let mut out = Vec::new();
        assert!(annexb_to_avcc(
            &annexb(&[aud, SPS, PPS, sei, IDR], false),
            &mut out
        ));
        assert_eq!(out, avcc(&[sei, IDR]));
    }

    #[test]
    fn config_only_packet_has_no_slice() {
        let mut out = Vec::new();
        assert!(!annexb_to_avcc(&annexb(&[SPS, PPS], true), &mut out));
        assert!(out.is_empty());
    }
}
