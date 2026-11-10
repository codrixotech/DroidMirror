//! Control threads: a writer that serializes `ControlMsg`s from a bounded
//! queue, and a reader for device messages (clipboard, call state).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread;

use anyhow::Result;

use crate::protocol::control::ControlMsg;
use crate::protocol::device_msg::{self, DeviceMsg};

/// Large enough for bursts of mouse moves; the sender never blocks.
const QUEUE_LEN: usize = 256;

/// Cheap, cloneable handle used by the UI and the video thread.
/// `None` in the queue tells the writer to stop.
#[derive(Clone)]
pub struct ControlSender(SyncSender<Option<ControlMsg>>);

impl ControlSender {
    /// Never blocks: if the writer is stuck, the message is dropped.
    pub fn send(&self, msg: ControlMsg) {
        match self.0.try_send(Some(msg)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => log::warn!("control queue full, dropping an input event"),
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    pub(crate) fn stop_writer(&self) {
        let _ = self.0.try_send(None);
    }
}

/// Starts the writer and reader threads for a scrcpy control socket.
pub fn start(
    socket: TcpStream,
    on_message: impl Fn(DeviceMsg) + Send + 'static,
) -> Result<ControlSender> {
    let writer = socket.try_clone()?;
    start_split(Box::new(writer), Box::new(socket), on_message)
}

/// Same, with separate streams for each direction (companion mode).
pub fn start_split(
    writer: Box<dyn Write + Send>,
    reader: Box<dyn Read + Send>,
    on_message: impl Fn(DeviceMsg) + Send + 'static,
) -> Result<ControlSender> {
    let (tx, rx) = sync_channel(QUEUE_LEN);
    thread::Builder::new()
        .name("control-write".into())
        .spawn(move || write_loop(writer, rx))?;
    thread::Builder::new()
        .name("control-read".into())
        .spawn(move || read_loop(reader, on_message))?;
    Ok(ControlSender(tx))
}

fn write_loop(mut socket: Box<dyn Write + Send>, rx: Receiver<Option<ControlMsg>>) {
    let mut buf = Vec::with_capacity(64);
    while let Ok(Some(msg)) = rx.recv() {
        buf.clear();
        msg.serialize(&mut buf);
        if let Err(e) = socket.write_all(&buf).and_then(|()| socket.flush()) {
            log::debug!("control socket closed: {e}");
            break;
        }
    }
}

/// Device messages must always be read, or the device side blocks.
fn read_loop(mut socket: Box<dyn Read + Send>, on_message: impl Fn(DeviceMsg)) {
    loop {
        match device_msg::read(&mut socket) {
            Ok(Some(msg)) => on_message(msg),
            Ok(None) => break,
            Err(e) => {
                log::debug!("device message stream ended: {e}");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;

    #[test]
    fn writes_messages_and_reads_clipboard() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut device, _) = listener.accept().unwrap();

        let (clip_tx, clip_rx) = mpsc::channel();
        let sender = start(client, move |m| {
            if let DeviceMsg::Clipboard(t) = m {
                clip_tx.send(t).unwrap();
            }
        })
        .unwrap();

        sender.send(ControlMsg::ResetVideo);
        sender.send(ControlMsg::BackOrScreenOn(
            crate::protocol::control::KeyAction::Down,
        ));
        let mut got = [0u8; 3];
        device.read_exact(&mut got).unwrap();
        assert_eq!(got, [17, 4, 0]);

        device.write_all(&[0, 0, 0, 0, 2, b'h', b'i']).unwrap();
        assert_eq!(clip_rx.recv().unwrap(), "hi");

        sender.stop_writer();
    }
}
