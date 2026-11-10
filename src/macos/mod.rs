//! All AppKit / AVFoundation / CoreMedia FFI lives in this module, behind
//! small safe types. Nothing outside `macos/` uses `unsafe`.

mod app;
mod bonjour;
mod clipboard;
mod display;
mod main_queue;
mod qr;
mod tls;
mod view;

pub use app::run;
