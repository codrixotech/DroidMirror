//! H.264 display without manual decoding: AVCC frames are wrapped in
//! `CMSampleBuffer`s marked "display immediately" and enqueued into an
//! `AVSampleBufferDisplayLayer`, which decodes them in hardware.

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use anyhow::{Result, bail, ensure};
use objc2::rc::Retained;
use objc2_av_foundation::{
    AVLayerVideoGravityResizeAspect, AVQueuedSampleBufferRendering,
    AVQueuedSampleBufferRenderingStatus, AVSampleBufferDisplayLayer, AVSampleBufferVideoRenderer,
};
use objc2_core_foundation::{CFDictionary, CFMutableDictionary, CFRetained, kCFBooleanTrue};
use objc2_core_media::{
    CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMSampleTimingInfo,
    CMVideoFormatDescriptionCreateFromH264ParameterSets, kCMBlockBufferAssureMemoryNowFlag,
    kCMSampleAttachmentKey_DisplayImmediately,
};

use crate::h264::AVCC_LENGTH_SIZE;
use crate::video::VideoSink;

/// Creates the layer (main thread). Its frame is managed by the view.
pub fn create_layer() -> Retained<AVSampleBufferDisplayLayer> {
    // SAFETY: plain object creation and property setters.
    unsafe {
        let layer = AVSampleBufferDisplayLayer::new();
        if let Some(gravity) = AVLayerVideoGravityResizeAspect {
            layer.setVideoGravity(gravity);
        }
        layer
    }
}

/// The layer's `sampleBufferRenderer`, owned by the video thread.
pub struct Renderer {
    renderer: Retained<AVSampleBufferVideoRenderer>,
    format: Option<CFRetained<CMFormatDescription>>,
}

// SAFETY: AVSampleBufferDisplayLayer.h: "sampleBufferRenderer allows the
// client to safely enqueue sample buffers from a background thread"
// (macOS 14+). `Renderer` is moved to the video thread and used only there.
unsafe impl Send for Renderer {}

impl Renderer {
    pub fn new(layer: &AVSampleBufferDisplayLayer) -> Self {
        // SAFETY: simple getter, available on macOS 14+.
        let renderer = unsafe { layer.sampleBufferRenderer() };
        Self {
            renderer,
            format: None,
        }
    }

    fn failed(&self) -> bool {
        // SAFETY: simple getter.
        unsafe { self.renderer.status() == AVQueuedSampleBufferRenderingStatus::Failed }
    }

    fn make_sample(&self, avcc: &[u8]) -> Result<CFRetained<CMSampleBuffer>> {
        let Some(format) = &self.format else {
            bail!("no format description yet");
        };
        let len = avcc.len();
        ensure!(len > 0, "empty frame");

        // SAFETY: CoreMedia allocates `len` bytes (AssureMemoryNow), we copy
        // the frame in, and the sample buffer retains the block buffer. All
        // out-pointers are checked before being wrapped.
        unsafe {
            let mut block: *mut CMBlockBuffer = ptr::null_mut();
            let status = CMBlockBuffer::create_with_memory_block(
                None,
                ptr::null_mut(),
                len,
                None,
                ptr::null(),
                0,
                len,
                kCMBlockBufferAssureMemoryNowFlag,
                NonNull::from(&mut block),
            );
            ensure!(
                status == 0,
                "CMBlockBufferCreateWithMemoryBlock failed ({status})"
            );
            let block = CFRetained::from_raw(NonNull::new(block).expect("null block buffer"));

            let src = NonNull::new(avcc.as_ptr() as *mut c_void).expect("non-null slice");
            let status = CMBlockBuffer::replace_data_bytes(src, &block, 0, len);
            ensure!(
                status == 0,
                "CMBlockBufferReplaceDataBytes failed ({status})"
            );

            let sizes = [len];
            let mut sample: *mut CMSampleBuffer = ptr::null_mut();
            let status = CMSampleBuffer::create_ready(
                None,
                Some(&block),
                Some(format),
                1,
                0,
                ptr::null::<CMSampleTimingInfo>(),
                1,
                sizes.as_ptr(),
                NonNull::from(&mut sample),
            );
            ensure!(status == 0, "CMSampleBufferCreateReady failed ({status})");
            let sample = CFRetained::from_raw(NonNull::new(sample).expect("null sample buffer"));

            // Per the header, kCMSampleAttachmentKey_* attachments are set
            // through the sample attachments array + CFDictionarySetValue.
            if let Some(attachments) = sample.sample_attachments_array(true) {
                let dict = attachments.value_at_index(0) as *const CFDictionary
                    as *const CFMutableDictionary;
                if let (Some(dict), Some(yes)) = (dict.as_ref(), kCFBooleanTrue) {
                    CFMutableDictionary::set_value(
                        Some(dict),
                        kCMSampleAttachmentKey_DisplayImmediately as *const _ as *const c_void,
                        yes as *const _ as *const c_void,
                    );
                }
            }
            Ok(sample)
        }
    }
}

impl VideoSink for Renderer {
    fn configure(&mut self, sps: &[u8], pps: &[u8]) -> Result<()> {
        ensure!(!sps.is_empty() && !pps.is_empty(), "empty parameter set");
        let pointers = [
            NonNull::new(sps.as_ptr() as *mut u8).expect("non-null"),
            NonNull::new(pps.as_ptr() as *mut u8).expect("non-null"),
        ];
        let sizes = [sps.len(), pps.len()];
        let mut format: *const CMFormatDescription = ptr::null();
        // SAFETY: two valid parameter sets with matching sizes; the output
        // is checked before being wrapped (Create rule: we own it).
        let status = unsafe {
            CMVideoFormatDescriptionCreateFromH264ParameterSets(
                None,
                2,
                NonNull::from(&pointers).cast(),
                NonNull::from(&sizes).cast(),
                AVCC_LENGTH_SIZE as i32,
                NonNull::from(&mut format),
            )
        };
        ensure!(
            status == 0,
            "CMVideoFormatDescriptionCreateFromH264ParameterSets failed ({status})"
        );
        let format =
            NonNull::new(format as *mut CMFormatDescription).expect("null format description");
        // SAFETY: returned with +1 retain count by a Create function.
        self.format = Some(unsafe { CFRetained::from_raw(format) });
        Ok(())
    }

    fn ready(&self) -> bool {
        // SAFETY: simple getter.
        !self.failed() && unsafe { self.renderer.isReadyForMoreMediaData() }
    }

    fn enqueue(&mut self, avcc: &[u8]) -> Result<()> {
        if self.failed() {
            self.flush();
            bail!("renderer failed (e.g. decoder reset after sleep)");
        }
        let sample = self.make_sample(avcc)?;
        // SAFETY: valid sample buffer; renderer is safe off the main thread.
        unsafe { self.renderer.enqueueSampleBuffer(&sample) };
        Ok(())
    }

    fn flush(&mut self) {
        // SAFETY: resets the renderer's queue and status.
        unsafe { self.renderer.flush() };
    }
}
