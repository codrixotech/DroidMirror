//! Event-driven hand-off from background threads to the main thread.
//!
//! A version-0 `CFRunLoopSource` is added to the main run loop in all
//! common modes. `post` pushes an event, signals the source and wakes the
//! run loop; the run loop then calls `perform` on the main thread. There
//! is no timer and no polling.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;

use objc2_core_foundation::{
    CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, kCFRunLoopCommonModes,
};
use objc2_foundation::MainThreadMarker;

use crate::session::{Ui, UiEvent};

static QUEUE: Mutex<VecDeque<UiEvent>> = Mutex::new(VecDeque::new());

type Handler = Box<dyn FnMut(UiEvent)>;

thread_local! {
    /// Only ever set on the main thread.
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

pub struct MainQueue {
    source: CFRetained<CFRunLoopSource>,
    run_loop: CFRetained<CFRunLoop>,
}

// SAFETY: the only operations used from other threads are
// `CFRunLoopSourceSignal` and `CFRunLoopWakeUp`, which CoreFoundation
// documents as thread-safe (that is their purpose). CF objects use atomic
// reference counting.
unsafe impl Send for MainQueue {}
unsafe impl Sync for MainQueue {}

impl MainQueue {
    /// Installs the source on the main run loop; `handler` runs on the
    /// main thread for every posted event.
    pub fn install(_mtm: MainThreadMarker, handler: impl FnMut(UiEvent) + 'static) -> Self {
        HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
        let mut context = CFRunLoopSourceContext {
            version: 0,
            info: ptr::null_mut(),
            retain: None,
            release: None,
            copyDescription: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform: Some(perform),
        };
        // SAFETY: `context` is a valid version-0 context; CF copies it.
        let source = unsafe { CFRunLoopSource::new(None, 0, &mut context) }
            .expect("CFRunLoopSourceCreate failed");
        let run_loop = CFRunLoop::main().expect("no main run loop");
        // SAFETY: reading an immutable CF constant.
        let modes = unsafe { kCFRunLoopCommonModes };
        run_loop.add_source(Some(&source), modes);
        Self { source, run_loop }
    }
}

impl Ui for MainQueue {
    fn post(&self, event: UiEvent) {
        QUEUE.lock().unwrap().push_back(event);
        self.source.signal();
        self.run_loop.wake_up();
    }
}

/// Runs on the main thread when the source fires.
unsafe extern "C-unwind" fn perform(_info: *mut c_void) {
    // Take the handler out so a nested run loop (e.g. a modal panel opened
    // by the handler) that fires the source again doesn't re-enter it;
    // queued events are then picked up by the loop below.
    let Some(mut handler) = HANDLER.with(|h| h.borrow_mut().take()) else {
        return;
    };
    loop {
        let batch: Vec<UiEvent> = QUEUE.lock().unwrap().drain(..).collect();
        if batch.is_empty() {
            break;
        }
        for event in batch {
            handler(event);
        }
    }
    HANDLER.with(|h| *h.borrow_mut() = Some(handler));
}
