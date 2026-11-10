//! Video reader loop: socket → H.264 parameter sets / AVCC frames → sink.
//!
//! Runs on the video thread. It never blocks on the display: when the
//! renderer can't keep up, frames are dropped until the next key frame (a
//! dropped delta frame would corrupt every frame that references it), and
//! `RESET_VIDEO` asks the device for a fresh key frame, at most once per
//! second.

use std::io::Read;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::h264::{self, ParamSets};
use crate::protocol::control::ControlMsg;
use crate::protocol::video::{Event, VideoStream};

/// Where decoded-for-display frames go (`AVSampleBufferDisplayLayer` on
/// macOS, a no-op in headless mode).
pub trait VideoSink: Send {
    /// New SPS/PPS: rebuild the format description.
    fn configure(&mut self, sps: &[u8], pps: &[u8]) -> Result<()>;
    /// `false` when the renderer's queue is full; the frame is then dropped.
    fn ready(&self) -> bool;
    /// Enqueue one AVCC frame. An error means the renderer failed; it has
    /// been flushed and needs a key frame.
    fn enqueue(&mut self, avcc: &[u8]) -> Result<()>;
    /// Discard pending frames (new connection or resync).
    fn flush(&mut self);
}

/// Sink for `--headless`: accepts everything, displays nothing.
pub struct NullSink;

impl VideoSink for NullSink {
    fn configure(&mut self, _: &[u8], _: &[u8]) -> Result<()> {
        Ok(())
    }
    fn ready(&self) -> bool {
        true
    }
    fn enqueue(&mut self, _: &[u8]) -> Result<()> {
        Ok(())
    }
    fn flush(&mut self) {}
}

/// Decides which frames to enqueue after a drop.
#[derive(Debug)]
pub struct Resync {
    waiting_for_key: bool,
    last_reset: Option<Instant>,
    min_reset_interval: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Enqueue,
    /// Skip this frame; send `RESET_VIDEO` if `reset` is set.
    Skip {
        reset: bool,
    },
}

impl Resync {
    pub fn new(min_reset_interval: Duration) -> Self {
        Self {
            // The first frame we display must be a key frame.
            waiting_for_key: true,
            last_reset: None,
            min_reset_interval,
        }
    }

    /// For each incoming frame. `ready` is the renderer's readiness.
    pub fn decide(&mut self, key_frame: bool, ready: bool, now: Instant) -> Decision {
        if !ready {
            self.waiting_for_key = true;
            return Decision::Skip {
                reset: self.reset_allowed(now),
            };
        }
        if self.waiting_for_key && !key_frame {
            return Decision::Skip {
                reset: self.reset_allowed(now),
            };
        }
        self.waiting_for_key = false;
        Decision::Enqueue
    }

    /// After a renderer failure or a format change.
    pub fn require_key_frame(&mut self) {
        self.waiting_for_key = true;
    }

    fn reset_allowed(&mut self, now: Instant) -> bool {
        let allowed = self
            .last_reset
            .is_none_or(|t| now.duration_since(t) >= self.min_reset_interval);
        if allowed {
            self.last_reset = Some(now);
        }
        allowed
    }
}

/// Things the video loop reports upward.
pub trait VideoEvents {
    fn video_size(&mut self, width: u32, height: u32);
    fn send_control(&mut self, msg: ControlMsg);
}

/// Reads the video socket until it ends. Returns `Ok` on a clean end.
pub fn run(
    socket: impl Read,
    sink: &mut dyn VideoSink,
    events: &mut dyn VideoEvents,
    log_stats_at: log::Level,
) -> Result<()> {
    let mut stream = VideoStream::new(socket);
    stream.read_codec()?;
    log::debug!("video codec: h264");

    sink.flush();
    let mut params = ParamSets::default();
    let mut configured = false;
    let mut resync = Resync::new(Duration::from_secs(1));
    let mut avcc = Vec::new();
    let mut stats = Stats::new(log_stats_at);

    while let Some(event) = stream.next_event()? {
        let packet = match event {
            Event::Session { width, height } => {
                log::info!("video size {width}x{height}");
                events.video_size(width, height);
                continue;
            }
            Event::Packet(p) => p,
        };
        stats.add(packet.data.len());

        // Config packets carry SPS/PPS; some encoders also repeat them
        // in front of key frames.
        let found = ParamSets::extract(&packet.data);
        if !found.is_empty() {
            let new = ParamSets {
                sps: found.sps.or_else(|| params.sps.clone()),
                pps: found.pps.or_else(|| params.pps.clone()),
            };
            let changed = !configured || new != params;
            params = new;
            if let (true, Some(sps), Some(pps)) = (changed, &params.sps, &params.pps) {
                match sink.configure(sps, pps) {
                    Ok(()) => {
                        log::debug!("format description updated");
                        configured = true;
                        resync.require_key_frame();
                    }
                    Err(e) => log::warn!("invalid parameter sets: {e:#}"),
                }
            }
        }
        if packet.config || !configured {
            continue;
        }

        if !h264::annexb_to_avcc(&packet.data, &mut avcc) {
            continue;
        }
        match resync.decide(packet.key_frame, sink.ready(), Instant::now()) {
            Decision::Enqueue => {
                if let Err(e) = sink.enqueue(&avcc) {
                    log::warn!("renderer error, resyncing: {e:#}");
                    resync.require_key_frame();
                    events.send_control(ControlMsg::ResetVideo);
                }
            }
            Decision::Skip { reset } => {
                stats.dropped += 1;
                if reset {
                    log::debug!("behind or waiting for a key frame: requesting one");
                    events.send_control(ControlMsg::ResetVideo);
                }
            }
        }
        stats.report_if_due();
    }
    Ok(())
}

struct Stats {
    level: log::Level,
    since: Instant,
    packets: u64,
    bytes: u64,
    dropped: u64,
}

impl Stats {
    fn new(level: log::Level) -> Self {
        Self {
            level,
            since: Instant::now(),
            packets: 0,
            bytes: 0,
            dropped: 0,
        }
    }

    fn add(&mut self, len: usize) {
        self.packets += 1;
        self.bytes += len as u64;
    }

    /// Checked per packet, not by a timer.
    fn report_if_due(&mut self) {
        let elapsed = self.since.elapsed();
        if elapsed < Duration::from_secs(1) {
            return;
        }
        let secs = elapsed.as_secs_f64();
        log::log!(
            self.level,
            "{:5.1} pkt/s  {:6.0} kbit/s  {} dropped",
            self.packets as f64 / secs,
            self.bytes as f64 * 8.0 / 1000.0 / secs,
            self.dropped
        );
        *self = Self::new(self.level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn first_frame_must_be_a_key_frame() {
        let mut r = Resync::new(Duration::from_secs(1));
        let now = t0();
        assert_eq!(r.decide(false, true, now), Decision::Skip { reset: true });
        assert_eq!(r.decide(true, true, now), Decision::Enqueue);
        assert_eq!(r.decide(false, true, now), Decision::Enqueue);
    }

    #[test]
    fn drops_until_key_frame_after_falling_behind() {
        let mut r = Resync::new(Duration::from_secs(1));
        let now = t0();
        assert_eq!(r.decide(true, true, now), Decision::Enqueue);
        // Renderer full: drop and ask for a key frame.
        assert_eq!(r.decide(false, false, now), Decision::Skip { reset: true });
        // Ready again, but delta frames are still skipped.
        assert_eq!(r.decide(false, true, now), Decision::Skip { reset: false });
        assert_eq!(r.decide(true, true, now), Decision::Enqueue);
        assert_eq!(r.decide(false, true, now), Decision::Enqueue);
    }

    #[test]
    fn reset_video_is_rate_limited_to_once_per_second() {
        let mut r = Resync::new(Duration::from_secs(1));
        let now = t0();
        assert_eq!(r.decide(false, false, now), Decision::Skip { reset: true });
        let later = now + Duration::from_millis(500);
        assert_eq!(
            r.decide(false, false, later),
            Decision::Skip { reset: false }
        );
        assert_eq!(
            r.decide(false, true, later),
            Decision::Skip { reset: false }
        );
        let much_later = now + Duration::from_millis(1000);
        assert_eq!(
            r.decide(false, true, much_later),
            Decision::Skip { reset: true }
        );
    }

    #[test]
    fn format_change_requires_key_frame() {
        let mut r = Resync::new(Duration::from_secs(1));
        let now = t0();
        assert_eq!(r.decide(true, true, now), Decision::Enqueue);
        r.require_key_frame();
        assert!(matches!(r.decide(false, true, now), Decision::Skip { .. }));
        assert_eq!(r.decide(true, true, now), Decision::Enqueue);
    }

    // --- whole loop -------------------------------------------------------

    #[derive(Default)]
    struct Recorder {
        configs: usize,
        frames: Vec<Vec<u8>>,
        ready: bool,
        flushes: usize,
    }

    impl VideoSink for Recorder {
        fn configure(&mut self, sps: &[u8], pps: &[u8]) -> Result<()> {
            assert_eq!(h264::nal_type(sps), h264::NAL_SPS);
            assert_eq!(h264::nal_type(pps), h264::NAL_PPS);
            self.configs += 1;
            Ok(())
        }
        fn ready(&self) -> bool {
            self.ready
        }
        fn enqueue(&mut self, avcc: &[u8]) -> Result<()> {
            self.frames.push(avcc.to_vec());
            Ok(())
        }
        fn flush(&mut self) {
            self.flushes += 1;
        }
    }

    #[derive(Default)]
    struct Events {
        sizes: Vec<(u32, u32)>,
        resets: usize,
    }

    impl VideoEvents for Events {
        fn video_size(&mut self, w: u32, h: u32) {
            self.sizes.push((w, h));
        }
        fn send_control(&mut self, msg: ControlMsg) {
            assert_eq!(msg, ControlMsg::ResetVideo);
            self.resets += 1;
        }
    }

    fn session(w: u32, h: u32) -> Vec<u8> {
        [0x8000_0000u32, w, h]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect()
    }

    fn media(config: bool, key: bool, data: &[u8]) -> Vec<u8> {
        let flags: u64 = if config {
            1 << 62
        } else if key {
            1 << 61
        } else {
            0
        };
        let mut v = flags.to_be_bytes().to_vec();
        v.extend_from_slice(&(data.len() as u32).to_be_bytes());
        v.extend_from_slice(data);
        v
    }

    const SPS: &[u8] = b"\0\0\0\x01\x67\x42\xc0\x1f";
    const PPS: &[u8] = b"\0\0\0\x01\x68\xce\x3c\x80";
    const IDR: &[u8] = b"\0\0\0\x01\x65\x88";
    const P: &[u8] = b"\0\0\0\x01\x41\x9a";

    #[test]
    fn full_stream_with_rotation() {
        let mut wire = b"h264".to_vec();
        wire.extend(session(472, 1024));
        wire.extend(media(true, false, &[SPS, PPS].concat()));
        wire.extend(media(false, true, IDR));
        wire.extend(media(false, false, P));
        // Rotation: new session, new SPS/PPS, new key frame.
        wire.extend(session(1024, 472));
        wire.extend(media(
            true,
            false,
            &[b"\0\0\0\x01\x67\x42\xc0\x20".as_slice(), PPS].concat(),
        ));
        wire.extend(media(false, false, P)); // stale delta: skipped
        wire.extend(media(false, true, IDR));

        let mut sink = Recorder {
            ready: true,
            ..Default::default()
        };
        let mut ev = Events::default();
        run(&wire[..], &mut sink, &mut ev, log::Level::Debug).unwrap();

        assert_eq!(ev.sizes, [(472, 1024), (1024, 472)]);
        assert_eq!(sink.configs, 2);
        assert_eq!(sink.flushes, 1);
        assert_eq!(sink.frames.len(), 3);
        assert_eq!(sink.frames[0], [0, 0, 0, 2, 0x65, 0x88]);
        assert_eq!(sink.frames[1], [0, 0, 0, 2, 0x41, 0x9a]);
        assert_eq!(ev.resets, 1); // for the stale delta frame
    }

    #[test]
    fn unchanged_parameter_sets_do_not_reconfigure() {
        let mut wire = b"h264".to_vec();
        wire.extend(media(true, false, &[SPS, PPS].concat()));
        wire.extend(media(false, true, &[SPS, PPS, IDR].concat())); // inline repeat
        wire.extend(media(false, false, P));
        let mut sink = Recorder {
            ready: true,
            ..Default::default()
        };
        run(
            &wire[..],
            &mut sink,
            &mut Events::default(),
            log::Level::Debug,
        )
        .unwrap();
        assert_eq!(sink.configs, 1);
        assert_eq!(sink.frames.len(), 2);
    }

    #[test]
    fn busy_renderer_drops_frames() {
        let mut wire = b"h264".to_vec();
        wire.extend(media(true, false, &[SPS, PPS].concat()));
        for _ in 0..5 {
            wire.extend(media(false, true, IDR));
        }
        let mut sink = Recorder {
            ready: false,
            ..Default::default()
        };
        let mut ev = Events::default();
        run(&wire[..], &mut sink, &mut ev, log::Level::Debug).unwrap();
        assert!(sink.frames.is_empty());
        assert_eq!(ev.resets, 1); // rate limited
    }
}
