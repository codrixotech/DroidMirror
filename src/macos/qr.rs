//! QR code rendering with CoreImage's built-in `CIQRCodeGenerator`.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{NSCIImageRep, NSImage};
use objc2_core_foundation::CGAffineTransform;
use objc2_core_image::CIFilter;
use objc2_foundation::{NSData, NSObjectNSKeyValueCoding, NSString, ns_string};

/// Pixels per QR module, so the image is sharp when shown at ~200 points.
const SCALE: f64 = 8.0;

pub fn image(text: &str) -> Option<Retained<NSImage>> {
    // SAFETY: documented filter name and input keys; values have the types
    // the filter expects (NSData message, NSString correction level).
    let ci = unsafe {
        let filter = CIFilter::filterWithName(ns_string!("CIQRCodeGenerator"))?;
        let data = NSData::with_bytes(text.as_bytes());
        filter.setValue_forKey(Some(&data), ns_string!("inputMessage"));
        filter.setValue_forKey(
            Some(&NSString::from_str("M")),
            ns_string!("inputCorrectionLevel"),
        );
        let small = filter.outputImage()?;
        // Nearest-neighbour scaling keeps the modules crisp.
        let scale = CGAffineTransform {
            a: SCALE,
            b: 0.0,
            c: 0.0,
            d: SCALE,
            tx: 0.0,
            ty: 0.0,
        };
        small
            .imageBySamplingNearest()
            .imageByApplyingTransform(scale)
    };
    let rep = NSCIImageRep::imageRepWithCIImage(&ci);
    let image = NSImage::initWithSize(NSImage::alloc(), rep.size());
    image.addRepresentation(&rep);
    Some(image)
}
