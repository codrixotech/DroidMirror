//! The phone-shaped window and the view that shows the video and turns
//! mouse, scroll and keyboard events into control messages.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSDragOperation, NSDraggingDestination, NSDraggingInfo, NSEvent,
    NSFont, NSPasteboardTypeFileURL, NSResponder, NSTextAlignment, NSTextField, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_av_foundation::AVSampleBufferDisplayLayer;
use objc2_foundation::{
    MainThreadMarker, NSArray, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
    ns_string,
};
use objc2_quartz_core::CATransaction;

use super::display;
use crate::controller::ControlSender;
use crate::input::{self, KeyInput, Rect};
use crate::protocol::control::{
    ControlMsg, KeyAction, POINTER_ID_GENERIC_FINGER, Position, TouchAction,
};

/// Dark border around the video, in points. The window is dragged by it.
pub const BEZEL: f64 = 10.0;

// ---------------------------------------------------------------------------
// Window

define_class!(
    // SAFETY: NSWindow may be subclassed; we only override two getters and
    // add no ivars or Drop.
    #[unsafe(super(NSWindow, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DMMirrorWindow"]
    pub struct MirrorWindow;

    impl MirrorWindow {
        /// Borderless windows can't become key (receive keyboard input)
        /// unless this is overridden.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            true
        }
    }
);

impl MirrorWindow {
    pub fn new(mtm: MainThreadMarker, size: NSSize) -> Retained<Self> {
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), size);
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::Resizable;
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: designated initializer of NSWindow with valid arguments.
        let window: Retained<Self> = unsafe {
            msg_send![super(this), initWithContentRect: rect, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false]
        };
        // SAFETY: we keep our own strong reference; required for windows
        // not owned by a window controller.
        unsafe { window.setReleasedWhenClosed(false) };
        // Transparent window so the view's rounded corners show; the view
        // draws the phone body.
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(true);
        window.center();
        window
    }
}

// ---------------------------------------------------------------------------
// View

type DropHandler = Rc<dyn Fn(Vec<PathBuf>)>;

pub struct ViewIvars {
    video_layer: Retained<AVSampleBufferDisplayLayer>,
    status: Retained<NSTextField>,
    input: RefCell<InputState>,
    on_drop: RefCell<Option<DropHandler>>,
}

#[derive(Default)]
struct InputState {
    control: Option<ControlSender>,
    /// Current video frame size (from the last session packet).
    frame: (u32, u32),
    touching: bool,
    /// Keys sent as key codes and still held: (mac key code, Android key code).
    keys_down: Vec<(u16, u32)>,
}

define_class!(
    // SAFETY: NSView may be subclassed; overridden methods have the
    // signatures AppKit expects, and the ivars need no special Drop.
    #[unsafe(super(NSView, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DMMirrorView"]
    #[ivars = ViewIvars]
    pub struct MirrorView;

    impl MirrorView {
        /// Top-left origin, like the video frame.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        /// A click on the inactive window is delivered as a touch too.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(setFrameSize:))]
        fn __set_frame_size(&self, size: NSSize) {
            // SAFETY: forwarding to NSView's implementation.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.layout_contents();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let p = self.event_point(event);
            if !self.video_rect().contains(p.0, p.1) {
                // The bezel moves the window.
                if let Some(window) = self.window() {
                    window.performWindowDragWithEvent(event);
                }
                return;
            }
            if self.send_touch(TouchAction::Down, p, false) {
                self.ivars().input.borrow_mut().touching = true;
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if self.ivars().input.borrow().touching {
                self.send_touch(TouchAction::Move, self.event_point(event), true);
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let touching = std::mem::take(&mut self.ivars().input.borrow_mut().touching);
            if touching {
                self.send_touch(TouchAction::Up, self.event_point(event), true);
            }
        }

        /// Right click = Back (or turn the screen on), like scrcpy.
        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, _event: &NSEvent) {
            self.send(ControlMsg::BackOrScreenOn(KeyAction::Down));
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, _event: &NSEvent) {
            self.send(ControlMsg::BackOrScreenOn(KeyAction::Up));
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let p = self.event_point(event);
            let Some(position) = self.position(p, false) else {
                return;
            };
            let precise = event.hasPreciseScrollingDeltas();
            let hscroll = -input::scroll_ticks(event.scrollingDeltaX(), precise);
            let vscroll = input::scroll_ticks(event.scrollingDeltaY(), precise);
            if hscroll != 0.0 || vscroll != 0.0 {
                self.send(ControlMsg::InjectScroll {
                    position,
                    hscroll,
                    vscroll,
                    buttons: 0,
                });
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let flags = event.modifierFlags().0 as u64;
            let mac = event.keyCode();
            let chars = event.characters().map(|s| s.to_string()).unwrap_or_default();
            match input::map_key_down(mac, flags, &chars) {
                KeyInput::Keycode(keycode) => {
                    let repeat = event.isARepeat() as u32;
                    {
                        let mut st = self.ivars().input.borrow_mut();
                        if !st.keys_down.iter().any(|(m, _)| *m == mac) {
                            st.keys_down.push((mac, keycode));
                        }
                    }
                    self.send(ControlMsg::InjectKeycode {
                        action: KeyAction::Down,
                        keycode,
                        repeat,
                        meta_state: input::meta_state(flags),
                    });
                }
                KeyInput::Text(text) => self.send(ControlMsg::InjectText(text)),
                KeyInput::Ignore => {}
            }
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            let mac = event.keyCode();
            let released = {
                let mut st = self.ivars().input.borrow_mut();
                st.keys_down
                    .iter()
                    .position(|(m, _)| *m == mac)
                    .map(|i| st.keys_down.remove(i).1)
            };
            if let Some(keycode) = released {
                self.send(ControlMsg::InjectKeycode {
                    action: KeyAction::Up,
                    keycode,
                    repeat: 0,
                    meta_state: input::meta_state(event.modifierFlags().0 as u64),
                });
            }
        }
    }

    unsafe impl NSObjectProtocol for MirrorView {}

    // Drag and drop files onto the window to push them to the phone.
    unsafe impl NSDraggingDestination for MirrorView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let paths = dropped_paths(sender);
            let callback = self.ivars().on_drop.borrow().clone();
            match callback {
                Some(cb) if !paths.is_empty() => {
                    cb(paths);
                    true
                }
                _ => false,
            }
        }
    }
);

/// File paths from a drag, via the `public.file-url` string of each item.
fn dropped_paths(sender: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
    // SAFETY: reading an immutable AppKit constant.
    let ty = unsafe { NSPasteboardTypeFileURL };
    let items = sender.draggingPasteboard().pasteboardItems();
    let Some(items) = items else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| item.stringForType(ty))
        .filter_map(|s| NSURL::URLWithString(&s))
        .filter_map(|url| url.path())
        .map(|p| PathBuf::from(p.to_string()))
        .collect()
}

impl MirrorView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let video_layer = display::create_layer();
        let status = NSTextField::wrappingLabelWithString(ns_string!(""), mtm);
        status.setTextColor(Some(&NSColor::whiteColor()));
        status.setAlignment(NSTextAlignment::Center);
        status.setFont(Some(&NSFont::systemFontOfSize(14.0)));
        status.setSelectable(false);

        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            video_layer,
            status,
            input: RefCell::default(),
            on_drop: RefCell::new(None),
        });
        // SAFETY: NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };

        this.setWantsLayer(true);
        if let Some(layer) = this.layer() {
            layer.setMasksToBounds(true);
            let body = NSColor::colorWithWhite_alpha(0.07, 1.0);
            layer.setBackgroundColor(Some(&body.CGColor()));
            layer.addSublayer(&this.ivars().video_layer);
        }
        this.ivars().video_layer.setMasksToBounds(true);
        this.addSubview(&this.ivars().status);

        // SAFETY: reading an immutable AppKit constant.
        let file_url = unsafe { NSPasteboardTypeFileURL };
        this.registerForDraggedTypes(&NSArray::from_slice(&[file_url]));

        this.layout_contents();
        this
    }

    pub fn video_layer(&self) -> &AVSampleBufferDisplayLayer {
        &self.ivars().video_layer
    }

    pub fn set_control(&self, control: Option<ControlSender>) {
        let mut st = self.ivars().input.borrow_mut();
        st.control = control;
        st.touching = false;
        st.keys_down.clear();
    }

    pub fn set_video_size(&self, width: u32, height: u32) {
        self.ivars().input.borrow_mut().frame = (width, height);
    }

    pub fn set_on_drop(&self, f: impl Fn(Vec<PathBuf>) + 'static) {
        *self.ivars().on_drop.borrow_mut() = Some(Rc::new(f));
    }

    /// Shows a message over a dimmed video.
    pub fn show_status(&self, text: &str) {
        let status = &self.ivars().status;
        status.setStringValue(&NSString::from_str(text));
        status.setHidden(false);
        self.ivars().video_layer.setOpacity(0.3);
        self.layout_contents();
    }

    pub fn hide_status(&self) {
        self.ivars().status.setHidden(true);
        self.ivars().video_layer.setOpacity(1.0);
    }

    /// Area inside the bezel.
    fn inner_rect(&self) -> Rect {
        let b = self.bounds();
        Rect {
            x: BEZEL,
            y: BEZEL,
            w: (b.size.width - 2.0 * BEZEL).max(0.0),
            h: (b.size.height - 2.0 * BEZEL).max(0.0),
        }
    }

    /// Where the video is actually drawn (aspect fit inside the bezel).
    fn video_rect(&self) -> Rect {
        let (w, h) = self.ivars().input.borrow().frame;
        input::fit_rect(self.inner_rect(), w, h)
    }

    fn layout_contents(&self) {
        let b = self.bounds();
        let inner = self.inner_rect();
        let radius = (b.size.width.min(b.size.height) * 0.11).min(54.0);

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        if let Some(layer) = self.layer() {
            layer.setCornerRadius(radius);
        }
        let video = &self.ivars().video_layer;
        video.setFrame(to_ns(inner));
        video.setCornerRadius((radius - BEZEL).max(0.0));
        CATransaction::commit();

        let status = &self.ivars().status;
        let max_w = (inner.w - 40.0).max(50.0);
        status.setPreferredMaxLayoutWidth(max_w);
        let h = status.fittingSize().height.min(inner.h);
        status.setFrame(NSRect::new(
            NSPoint::new(
                inner.x + (inner.w - max_w) / 2.0,
                inner.y + (inner.h - h) / 2.0,
            ),
            NSSize::new(max_w, h),
        ));
    }

    fn event_point(&self, event: &NSEvent) -> (f64, f64) {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        (p.x, p.y)
    }

    fn position(&self, p: (f64, f64), clamp: bool) -> Option<Position> {
        let (fw, fh) = self.ivars().input.borrow().frame;
        let (x, y) = input::map_point(self.video_rect(), fw, fh, p.0, p.1, clamp)?;
        Some(Position {
            x,
            y,
            width: u16::try_from(fw).ok()?,
            height: u16::try_from(fh).ok()?,
        })
    }

    /// Returns whether a message was sent.
    fn send_touch(&self, action: TouchAction, p: (f64, f64), clamp: bool) -> bool {
        let Some(position) = self.position(p, clamp) else {
            return false;
        };
        self.send(ControlMsg::InjectTouch {
            action,
            pointer_id: POINTER_ID_GENERIC_FINGER,
            position,
            pressure: if action == TouchAction::Up { 0.0 } else { 1.0 },
            action_button: 0,
            buttons: 0,
        });
        true
    }

    pub fn send(&self, msg: ControlMsg) {
        if let Some(c) = &self.ivars().input.borrow().control {
            c.send(msg);
        }
    }
}

fn to_ns(r: Rect) -> NSRect {
    NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
}
