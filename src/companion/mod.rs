//! Companion mode: mirror a phone running the DroidMirror Android app,
//! without Developer options or adb.
//!
//! The app advertises `_droidmirror._tcp` on the local network and runs a
//! TLS server with its own self-signed certificate. Every connection starts
//! with a hello from the Mac (after the TLS handshake):
//!
//! ```text
//! "DMC1" | channel u8 | payload
//!   PAIR    (3): nonce[16] | name_len u8 | name     → status | token[32] | id_len u8 | id
//!   VIDEO   (0): token[32]                          → status | then 64-byte device name +
//!                                                     the scrcpy video stream format
//!   CONTROL (1): token[32]                          → status | then scrcpy control messages
//!   EVENTS  (2): token[32]                          → status | then device messages
//! status: 0 ok, 1 denied, 2 unknown token, 3 not sharing
//! ```
//!
//! Pairing works like Bluetooth numeric comparison: both sides show a
//! 6-digit code derived from the phone's certificate and the Mac's nonce,
//! and the user taps Allow on the phone if they match. The Mac then pins
//! the certificate fingerprint and keeps the token the phone issued.

pub mod store;

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::sha256;
pub use store::{PairingRecord, PairingStore};

pub const SERVICE_TYPE: &str = "_droidmirror._tcp.";
const MAGIC: &[u8; 4] = b"DMC1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Channel {
    Video = 0,
    Control = 1,
    Events = 2,
    Pair = 3,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CompanionError {
    #[error("pairing was declined on the phone")]
    Denied,
    #[error("the phone no longer knows this Mac; pairing again")]
    UnknownToken,
    #[error("open DroidMirror on the phone and tap Start sharing")]
    NotSharing,
    #[error("unexpected reply {0} from the phone")]
    Protocol(u8),
}

fn check_status(status: u8) -> Result<(), CompanionError> {
    match status {
        0 => Ok(()),
        1 => Err(CompanionError::Denied),
        2 => Err(CompanionError::UnknownToken),
        3 => Err(CompanionError::NotSharing),
        other => Err(CompanionError::Protocol(other)),
    }
}

/// A phone found on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// Display name (the mDNS instance name).
    pub name: String,
    /// Stable id from the TXT record.
    pub id: String,
    pub addr: SocketAddr,
}

pub trait Discovery: Send + Sync {
    fn peers(&self) -> Vec<Peer>;
}

pub trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

pub struct TlsConn {
    pub stream: Box<dyn Stream>,
    /// Underlying socket, kept to shut the connection down from another thread.
    pub tcp: TcpStream,
    /// SHA-256 of the server's leaf certificate (DER).
    pub fingerprint: [u8; 32],
}

impl std::fmt::Debug for TlsConn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsConn")
            .field("tcp", &self.tcp)
            .finish_non_exhaustive()
    }
}

pub trait TlsConnector: Send + Sync {
    /// Connects with TLS. With `pin`, the handshake fails unless the
    /// server certificate's SHA-256 matches.
    fn connect(&self, addr: SocketAddr, pin: Option<&[u8; 32]>) -> Result<TlsConn>;
}

/// Everything companion mode needs from the platform.
#[derive(Clone)]
pub struct Companion {
    pub tls: Arc<dyn TlsConnector>,
    pub discovery: Arc<dyn Discovery>,
    pub store: Arc<PairingStore>,
    /// Shown on the phone when pairing ("MacBook Air").
    pub mac_name: String,
}

/// Code both sides display: SHA-256(fingerprint || nonce), first 4 bytes,
/// as 6 decimal digits.
pub fn pairing_code(fingerprint: &[u8; 32], nonce: &[u8; 16]) -> u32 {
    let mut data = fingerprint.to_vec();
    data.extend_from_slice(nonce);
    let h = sha256::digest(&data);
    u32::from_be_bytes([h[0], h[1], h[2], h[3]]) % 1_000_000
}

pub fn format_code(code: u32) -> String {
    format!("{:03} {:03}", code / 1000, code % 1000)
}

pub fn random_bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf)
}

fn hello(channel: Channel, payload: &[u8]) -> Vec<u8> {
    let mut v = MAGIC.to_vec();
    v.push(channel as u8);
    v.extend_from_slice(payload);
    v
}

const PAIR_TIMEOUT: Duration = Duration::from_secs(75);
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Pairs with a phone. `show_code` is called with the code to display
/// while the user confirms on the phone. `register` receives the socket so
/// the caller can abort the wait.
pub fn pair(
    c: &Companion,
    peer: &Peer,
    show_code: &dyn Fn(&str),
    register: &dyn Fn(&TcpStream),
) -> Result<PairingRecord> {
    let mut conn = c.tls.connect(peer.addr, None)?;
    register(&conn.tcp);
    let nonce: [u8; 16] = random_bytes()?;
    let name = crate::protocol::control::truncate_utf8(&c.mac_name, 255);
    let mut payload = nonce.to_vec();
    payload.push(name.len() as u8);
    payload.extend_from_slice(name.as_bytes());
    conn.stream.write_all(&hello(Channel::Pair, &payload))?;
    conn.stream.flush()?;

    show_code(&format_code(pairing_code(&conn.fingerprint, &nonce)));

    conn.tcp.set_read_timeout(Some(PAIR_TIMEOUT))?;
    let mut status = [0u8; 1];
    conn.stream
        .read_exact(&mut status)
        .context("no answer from the phone")?;
    check_status(status[0])?;
    let mut token = [0u8; 32];
    conn.stream.read_exact(&mut token)?;
    let mut len = [0u8; 1];
    conn.stream.read_exact(&mut len)?;
    let mut id = vec![0u8; len[0] as usize];
    conn.stream.read_exact(&mut id)?;
    let id = String::from_utf8_lossy(&id).into_owned();
    if id != peer.id {
        bail!("the phone's id changed during pairing");
    }
    let record = PairingRecord {
        id,
        name: peer.name.clone(),
        fingerprint: conn.fingerprint,
        token,
    };
    c.store.save(&record)?;
    Ok(record)
}

/// Opens an authenticated channel to a paired phone.
pub fn open(
    c: &Companion,
    peer: &Peer,
    record: &PairingRecord,
    channel: Channel,
) -> Result<TlsConn> {
    let mut conn = c
        .tls
        .connect(peer.addr, Some(&record.fingerprint))
        .context("secure connection to the phone failed")?;
    conn.stream.write_all(&hello(channel, &record.token))?;
    conn.stream.flush()?;
    conn.tcp.set_read_timeout(Some(REPLY_TIMEOUT))?;
    let mut status = [0u8; 1];
    conn.stream.read_exact(&mut status)?;
    conn.tcp.set_read_timeout(None)?;
    check_status(status[0])?;
    Ok(conn)
}

#[cfg(test)]
mod tests;
