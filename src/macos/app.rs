//! Application setup: menus, the app delegate, and the main-thread side of
//! the session (status, resizing on rotation, clipboard, shutdown).

use std::cell::RefCell;
use std::sync::Arc;

use anyhow::Result;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSMenu, NSMenuItem,
    NSRequestUserAttentionType, NSScreen, NSWindow, NSWindowDelegate,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, ns_string,
};

use super::clipboard;
use super::display::Renderer;
use super::main_queue::MainQueue;
use super::view::{BEZEL, MirrorView, MirrorWindow};
use crate::adb::Adb;
use crate::call::{self, CallState};
use crate::controller::ControlSender;
use crate::input::{self, keycode};
use crate::protocol::control::{ControlMsg, CopyKey, KeyAction};
use crate::session::{Config, Session, UiEvent};

/// Runs the app; returns when it quits.
pub fn run(adb: Adb, config: Config) -> Result<()> {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate = AppDelegate::new(mtm, adb, config);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
    Ok(())
}

struct AppIvars {
    adb: Adb,
    config: Config,
    state: RefCell<Option<AppState>>,
}

struct AppState {
    window: Retained<MirrorWindow>,
    view: Retained<MirrorView>,
    session: Option<Session>,
    control: Option<ControlSender>,
    /// Current video frame size.
    video: (u32, u32),
    /// Pasteboard change count already synced to the phone (or set by it).
    pasteboard_seen: isize,
    call: CallState,
    /// Dock bounce request while ringing.
    attention: Option<isize>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DMAppDelegate"]
    #[ivars = AppIvars]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            self.launch();
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _app: &NSApplication) -> bool {
            true
        }

        /// Clean shutdown: stop the server before the process exits
        /// (`terminate:` calls `exit()`, so no Rust destructors run).
        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            let session = self.ivars().state.borrow_mut().as_mut().and_then(|s| s.session.take());
            if let Some(mut session) = session {
                session.stop();
            }
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn did_become_active(&self, _notification: &NSNotification) {
            self.sync_clipboard_to_device(false);
        }
    }

    unsafe impl NSWindowDelegate for AppDelegate {
        /// Keeps the video aspect ratio during a live resize.
        #[unsafe(method(windowWillResize:toSize:))]
        fn window_will_resize(&self, _window: &NSWindow, size: NSSize) -> NSSize {
            let video = self.ivars().state.borrow().as_ref().map(|s| s.video);
            match video {
                Some((w, h)) if w > 0 && h > 0 => {
                    let (nw, nh) = input::constrain_resize(size.width, size.height, w, h, BEZEL);
                    NSSize::new(nw, nh)
                }
                _ => size,
            }
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            // The shadow follows the rounded shape only once recomputed.
            if let Some(window) = self.window() {
                window.invalidateShadow();
            }
        }
    }

    // Menu actions.
    impl AppDelegate {
        #[unsafe(method(pressBack:))]
        fn press_back(&self, _sender: Option<&AnyObject>) {
            self.press_key(keycode::BACK);
        }

        #[unsafe(method(pressHome:))]
        fn press_home(&self, _sender: Option<&AnyObject>) {
            self.press_key(keycode::HOME);
        }

        #[unsafe(method(pressRecents:))]
        fn press_recents(&self, _sender: Option<&AnyObject>) {
            self.press_key(keycode::APP_SWITCH);
        }

        /// Cmd+C: copy the selection on the phone; the phone then sends
        /// its clipboard, which lands in the Mac clipboard.
        #[unsafe(method(copyFromDevice:))]
        fn copy_from_device(&self, _sender: Option<&AnyObject>) {
            self.send(ControlMsg::GetClipboard(CopyKey::Copy));
        }

        /// Answer button / Cmd+Return. Only while ringing.
        #[unsafe(method(answerCall:))]
        fn answer_call(&self, _sender: Option<&AnyObject>) {
            if matches!(self.call_state(), CallState::Ringing { .. }) {
                log::info!("answering call");
                self.press_key(call::KEYCODE_CALL);
            }
        }

        /// Decline / End button / Cmd+Delete. Only during a call: when
        /// idle, KEYCODE_ENDCALL would turn the screen off instead.
        #[unsafe(method(declineCall:))]
        fn decline_call(&self, _sender: Option<&AnyObject>) {
            if self.call_state() != CallState::Idle {
                log::info!("ending call");
                self.press_key(call::KEYCODE_ENDCALL);
            }
        }

        /// Cmd+V: paste the Mac clipboard on the phone.
        #[unsafe(method(pasteToDevice:))]
        fn paste_to_device(&self, _sender: Option<&AnyObject>) {
            self.sync_clipboard_to_device(true);
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker, adb: Adb, config: Config) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppIvars {
            adb,
            config,
            state: RefCell::new(None),
        });
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }

    fn launch(&self) {
        let mtm = self.mtm();
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.setMainMenu(Some(&self.build_menu(mtm)));

        let window = MirrorWindow::new(mtm, NSSize::new(360.0, 760.0));
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            window.contentRectForFrameRect(window.frame()).size,
        );
        let view = MirrorView::new(mtm, frame);
        window.setContentView(Some(&view));
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        window.setTitle(ns_string!("DroidMirror"));
        window.makeKeyAndOrderFront(None);
        window.makeFirstResponder(Some(&view));
        view.show_status("Looking for an Android device…");

        // File drops go to the session, which pushes them with adb.
        let me = self.retain();
        view.set_on_drop(move |paths| {
            let state = me.ivars().state.borrow();
            if let Some(state) = state.as_ref()
                && let Some(session) = &state.session
            {
                log::info!("pushing {} file(s) to /sdcard/Download", paths.len());
                session.push_files(paths, state.control.clone());
            }
        });

        let me = self.retain();
        let queue = Arc::new(MainQueue::install(mtm, move |event| me.handle(event)));
        let renderer = Renderer::new(view.video_layer());
        let session = match Session::start(
            self.ivars().adb.clone(),
            self.ivars().config.clone(),
            Box::new(renderer),
            queue,
        ) {
            Ok(s) => Some(s),
            Err(e) => {
                view.show_status(&format!("{e:#}"));
                None
            }
        };

        *self.ivars().state.borrow_mut() = Some(AppState {
            window,
            view,
            session,
            control: None,
            video: (0, 0),
            pasteboard_seen: clipboard::change_count(),
            call: CallState::Idle,
            attention: None,
        });

        #[allow(deprecated)] // `activate` needs macOS 14 bundles; this works unbundled
        app.activateIgnoringOtherApps(true);
    }

    fn build_menu(&self, mtm: MainThreadMarker) -> Retained<NSMenu> {
        let bar = NSMenu::new(mtm);

        let app_menu = NSMenu::new(mtm);
        app_menu.addItem(&item(
            mtm,
            "About DroidMirror",
            Some(sel!(orderFrontStandardAboutPanel:)),
            "",
            None,
        ));
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        // No "Hide" item: Cmd+H is Home.
        app_menu.addItem(&item(
            mtm,
            "Quit DroidMirror",
            Some(sel!(terminate:)),
            "q",
            None,
        ));
        add_submenu(mtm, &bar, "DroidMirror", &app_menu);

        let edit = NSMenu::new(mtm);
        edit.addItem(&item(
            mtm,
            "Copy from Phone",
            Some(sel!(copyFromDevice:)),
            "c",
            Some(self),
        ));
        edit.addItem(&item(
            mtm,
            "Paste to Phone",
            Some(sel!(pasteToDevice:)),
            "v",
            Some(self),
        ));
        add_submenu(mtm, &bar, "Edit", &edit);

        let device = NSMenu::new(mtm);
        device.addItem(&item(mtm, "Back", Some(sel!(pressBack:)), "b", Some(self)));
        device.addItem(&item(mtm, "Home", Some(sel!(pressHome:)), "h", Some(self)));
        device.addItem(&item(
            mtm,
            "Recent Apps",
            Some(sel!(pressRecents:)),
            "s",
            Some(self),
        ));
        device.addItem(&NSMenuItem::separatorItem(mtm));
        device.addItem(&item(
            mtm,
            "Answer Call",
            Some(sel!(answerCall:)),
            "\r",
            Some(self),
        ));
        device.addItem(&item(
            mtm,
            "Decline / End Call",
            Some(sel!(declineCall:)),
            "\u{8}",
            Some(self),
        ));
        add_submenu(mtm, &bar, "Device", &device);

        bar
    }

    fn window(&self) -> Option<Retained<MirrorWindow>> {
        self.ivars()
            .state
            .borrow()
            .as_ref()
            .map(|s| s.window.clone())
    }

    fn view(&self) -> Option<Retained<MirrorView>> {
        self.ivars().state.borrow().as_ref().map(|s| s.view.clone())
    }

    fn send(&self, msg: ControlMsg) {
        if let Some(c) = self
            .ivars()
            .state
            .borrow()
            .as_ref()
            .and_then(|s| s.control.as_ref())
        {
            c.send(msg);
        }
    }

    fn press_key(&self, keycode: u32) {
        for action in [KeyAction::Down, KeyAction::Up] {
            self.send(ControlMsg::InjectKeycode {
                action,
                keycode,
                repeat: 0,
                meta_state: 0,
            });
        }
    }

    /// Sends the Mac clipboard to the phone if it changed since last time
    /// (or always, when pasting).
    fn sync_clipboard_to_device(&self, paste: bool) {
        let count = clipboard::change_count();
        let changed = {
            let mut state = self.ivars().state.borrow_mut();
            let Some(state) = state.as_mut() else { return };
            if state.control.is_none() {
                return;
            }
            let changed = state.pasteboard_seen != count;
            state.pasteboard_seen = count;
            changed
        };
        if !(changed || paste) {
            return;
        }
        if let Some(text) = clipboard::get_text() {
            self.send(ControlMsg::SetClipboard {
                sequence: 0, // no acknowledgement needed
                paste,
                text,
            });
        }
    }

    /// Main-thread handler for session events. Borrows of `state` are kept
    /// short: AppKit calls below can re-enter the delegate.
    fn handle(&self, event: UiEvent) {
        let (Some(window), Some(view)) = (self.window(), self.view()) else {
            return;
        };
        match event {
            UiEvent::Status(text) => view.show_status(&text),
            UiEvent::Connected {
                device_name,
                control,
            } => {
                window.setTitle(&NSString::from_str(&device_name));
                view.set_control(Some(control.clone()));
                if let Some(state) = self.ivars().state.borrow_mut().as_mut() {
                    state.control = Some(control);
                }
                view.hide_status();
                self.sync_clipboard_to_device(false);
            }
            UiEvent::VideoSize { width, height } => {
                view.set_video_size(width, height);
                let first = {
                    let mut state = self.ivars().state.borrow_mut();
                    let Some(state) = state.as_mut() else { return };
                    let first = state.video == (0, 0);
                    state.video = (width, height);
                    first
                };
                self.fit_window(&window, width, height, first);
            }
            UiEvent::Disconnected => {
                self.on_call_state(&window, &view, CallState::Idle);
                view.set_control(None);
                if let Some(state) = self.ivars().state.borrow_mut().as_mut() {
                    state.control = None;
                }
                view.show_status("Disconnected");
            }
            UiEvent::Clipboard(text) => {
                if clipboard::get_text().as_deref() != Some(text.as_str()) {
                    let count = clipboard::set_text(&text);
                    if let Some(state) = self.ivars().state.borrow_mut().as_mut() {
                        state.pasteboard_seen = count;
                    }
                }
            }
            UiEvent::Fatal(text) => view.show_status(&text),
            UiEvent::Notice(text) => view.show_toast(&text),
            UiEvent::Call(state) => self.on_call_state(&window, &view, state),
        }
    }

    fn call_state(&self) -> CallState {
        self.ivars()
            .state
            .borrow()
            .as_ref()
            .map_or(CallState::Idle, |s| s.call.clone())
    }

    fn on_call_state(&self, window: &MirrorWindow, view: &MirrorView, state: CallState) {
        let app = NSApplication::sharedApplication(self.mtm());
        let previous_attention = {
            let mut st = self.ivars().state.borrow_mut();
            let Some(st) = st.as_mut() else { return };
            st.call = state.clone();
            st.attention.take()
        };
        if let Some(id) = previous_attention {
            app.cancelUserAttentionRequest(id);
        }
        view.show_call(&state);
        if matches!(state, CallState::Ringing { .. }) {
            // Bring the phone into view without stealing keyboard focus,
            // and bounce the Dock icon until the user looks.
            if window.isMiniaturized() {
                window.deminiaturize(None);
            }
            window.orderFrontRegardless();
            let id = app.requestUserAttention(NSRequestUserAttentionType::CriticalRequest);
            if let Some(st) = self.ivars().state.borrow_mut().as_mut() {
                st.attention = Some(id);
            }
        }
    }

    /// Resizes the window for a new video size, keeping its long side
    /// (so a rotation swaps width and height) and its center.
    fn fit_window(&self, window: &MirrorWindow, width: u32, height: u32, first: bool) {
        let mtm = self.mtm();
        let Some(screen) = window.screen().or_else(|| NSScreen::mainScreen(mtm)) else {
            return;
        };
        let visible = screen.visibleFrame();
        let current = window.frame();
        let long = if first {
            visible.size.height * 0.8
        } else {
            current.size.width.max(current.size.height)
        };
        let max_w = (visible.size.width * 0.95).min(long);
        let max_h = (visible.size.height * 0.92).min(long);
        let (w, h) = input::window_size(width, height, max_w, max_h, BEZEL);

        let center_x = current.origin.x + current.size.width / 2.0;
        let center_y = current.origin.y + current.size.height / 2.0;
        let x = (center_x - w / 2.0)
            .max(visible.origin.x)
            .min(visible.origin.x + visible.size.width - w);
        let y = (center_y - h / 2.0)
            .max(visible.origin.y)
            .min(visible.origin.y + visible.size.height - h);
        let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        window.setFrame_display_animate(frame, true, !first);
        window.invalidateShadow();
    }
}

fn item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
    target: Option<&AppDelegate>,
) -> Retained<NSMenuItem> {
    // SAFETY: standard initializer; the action exists on the target (or on
    // NSApplication / the responder chain when there is no target).
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    };
    if let Some(target) = target {
        // SAFETY: the delegate outlives the menu (both live until exit).
        unsafe { item.setTarget(Some(target)) };
    }
    item
}

fn add_submenu(mtm: MainThreadMarker, bar: &NSMenu, title: &str, menu: &NSMenu) {
    menu.setTitle(&NSString::from_str(title));
    let holder = item(mtm, title, None, "", None);
    holder.setSubmenu(Some(menu));
    bar.addItem(&holder);
}
