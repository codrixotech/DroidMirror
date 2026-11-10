//! TLS client for companion mode, on Apple's Security framework
//! (SecureTransport). The handshake pauses after the server sends its
//! certificate (`kSSLSessionOptionBreakOnServerAuth`); we hash the leaf
//! certificate and either pin-check it or report it for pairing.
//!
//! SecureTransport is deprecated in favour of Network.framework, but it is
//! still shipped and supported up to TLS 1.2, and unlike Network.framework
//! it needs no Objective-C blocks. Each `TlsStream` is used by one thread.

use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::ptr;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::companion::{TlsConn, TlsConnector};
use crate::sha256;

type OSStatus = i32;
type SSLContextRef = *mut c_void;
type SSLConnectionRef = *const c_void;
type CFTypeRef = *const c_void;

const K_SSL_CLIENT_SIDE: i32 = 1;
const K_SSL_STREAM_TYPE: i32 = 0;
const K_SSL_SESSION_OPTION_BREAK_ON_SERVER_AUTH: i32 = 0;
const K_TLS_PROTOCOL_12: i32 = 8;

const ERR_SSL_WOULD_BLOCK: OSStatus = -9803;
const ERR_SSL_CLOSED_GRACEFUL: OSStatus = -9805;
const ERR_SSL_CLOSED_ABORT: OSStatus = -9806;
const ERR_SSL_CLOSED_NO_NOTIFY: OSStatus = -9816;
const ERR_SSL_SERVER_AUTH_COMPLETED: OSStatus = -9841;

type SSLReadFunc = unsafe extern "C" fn(SSLConnectionRef, *mut c_void, *mut usize) -> OSStatus;
type SSLWriteFunc = unsafe extern "C" fn(SSLConnectionRef, *const c_void, *mut usize) -> OSStatus;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SSLCreateContext(alloc: CFTypeRef, side: i32, kind: i32) -> SSLContextRef;
    fn SSLSetIOFuncs(ctx: SSLContextRef, read: SSLReadFunc, write: SSLWriteFunc) -> OSStatus;
    fn SSLSetConnection(ctx: SSLContextRef, connection: SSLConnectionRef) -> OSStatus;
    fn SSLSetSessionOption(ctx: SSLContextRef, option: i32, value: u8) -> OSStatus;
    fn SSLSetProtocolVersionMin(ctx: SSLContextRef, version: i32) -> OSStatus;
    fn SSLHandshake(ctx: SSLContextRef) -> OSStatus;
    fn SSLCopyPeerTrust(ctx: SSLContextRef, trust: *mut *mut c_void) -> OSStatus;
    fn SSLRead(
        ctx: SSLContextRef,
        data: *mut c_void,
        len: usize,
        processed: *mut usize,
    ) -> OSStatus;
    fn SSLWrite(
        ctx: SSLContextRef,
        data: *const c_void,
        len: usize,
        processed: *mut usize,
    ) -> OSStatus;
    fn SSLClose(ctx: SSLContextRef) -> OSStatus;
    fn SecTrustCopyCertificateChain(trust: *mut c_void) -> CFTypeRef;
    fn SecCertificateCopyData(cert: CFTypeRef) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFArrayGetCount(array: CFTypeRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFDataGetLength(data: CFTypeRef) -> isize;
}

/// Read callback: fill exactly `*len` bytes from the socket if possible.
unsafe extern "C" fn read_cb(
    conn: SSLConnectionRef,
    data: *mut c_void,
    len: *mut usize,
) -> OSStatus {
    // SAFETY: `conn` is the `Box<TcpStream>` registered in `connect`, alive
    // for the context's lifetime; SecureTransport passes a buffer of `*len`.
    let (tcp, buf) = unsafe {
        (
            &*(conn as *const TcpStream),
            std::slice::from_raw_parts_mut(data as *mut u8, *len),
        )
    };
    let mut done = 0;
    let status = loop {
        if done == buf.len() {
            break 0;
        }
        match (&*tcp).read(&mut buf[done..]) {
            Ok(0) => break ERR_SSL_CLOSED_GRACEFUL,
            Ok(n) => done += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break ERR_SSL_WOULD_BLOCK;
            }
            Err(_) => break ERR_SSL_CLOSED_ABORT,
        }
    };
    // SAFETY: valid out-pointer from SecureTransport.
    unsafe { *len = done };
    status
}

unsafe extern "C" fn write_cb(
    conn: SSLConnectionRef,
    data: *const c_void,
    len: *mut usize,
) -> OSStatus {
    // SAFETY: as in `read_cb`.
    let (tcp, buf) = unsafe {
        (
            &*(conn as *const TcpStream),
            std::slice::from_raw_parts(data as *const u8, *len),
        )
    };
    let mut done = 0;
    let status = loop {
        if done == buf.len() {
            break 0;
        }
        match (&*tcp).write(&buf[done..]) {
            Ok(0) => break ERR_SSL_CLOSED_ABORT,
            Ok(n) => done += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break ERR_SSL_WOULD_BLOCK;
            }
            Err(_) => break ERR_SSL_CLOSED_ABORT,
        }
    };
    // SAFETY: valid out-pointer from SecureTransport.
    unsafe { *len = done };
    status
}

pub struct TlsStream {
    ctx: SSLContextRef,
    /// Boxed so its address (given to SecureTransport) never moves.
    _tcp: Box<TcpStream>,
}

// SAFETY: the context and socket are owned by this value and only used
// through `&mut self`, i.e. by one thread at a time.
unsafe impl Send for TlsStream {}

impl Drop for TlsStream {
    fn drop(&mut self) {
        // SAFETY: valid context created by SSLCreateContext (+1 retain).
        unsafe {
            SSLClose(self.ctx);
            CFRelease(self.ctx as CFTypeRef);
        }
    }
}

impl Read for TlsStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut n = 0;
        // SAFETY: valid context and buffer.
        let status =
            unsafe { SSLRead(self.ctx, buf.as_mut_ptr() as *mut c_void, buf.len(), &mut n) };
        match status {
            _ if n > 0 => Ok(n),
            0 => Ok(0),
            ERR_SSL_CLOSED_GRACEFUL | ERR_SSL_CLOSED_NO_NOTIFY => Ok(0),
            ERR_SSL_WOULD_BLOCK => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "TLS read timed out",
            )),
            s => Err(io::Error::other(format!("TLS read failed ({s})"))),
        }
    }
}

impl Write for TlsStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut n = 0;
        // SAFETY: valid context and buffer.
        let status =
            unsafe { SSLWrite(self.ctx, buf.as_ptr() as *const c_void, buf.len(), &mut n) };
        match status {
            _ if n > 0 => Ok(n),
            ERR_SSL_WOULD_BLOCK => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "TLS write timed out",
            )),
            s => Err(io::Error::other(format!("TLS write failed ({s})"))),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(()) // SSLWrite hands data to the socket synchronously.
    }
}

/// SHA-256 of the server's leaf certificate.
fn peer_fingerprint(ctx: SSLContextRef) -> Result<[u8; 32]> {
    // SAFETY: CF objects are checked for null and released exactly once.
    unsafe {
        let mut trust: *mut c_void = ptr::null_mut();
        let status = SSLCopyPeerTrust(ctx, &mut trust);
        if status != 0 || trust.is_null() {
            bail!("no server certificate ({status})");
        }
        let chain = SecTrustCopyCertificateChain(trust);
        CFRelease(trust as CFTypeRef);
        if chain.is_null() || CFArrayGetCount(chain) < 1 {
            if !chain.is_null() {
                CFRelease(chain);
            }
            bail!("empty certificate chain");
        }
        let data = SecCertificateCopyData(CFArrayGetValueAtIndex(chain, 0));
        CFRelease(chain);
        if data.is_null() {
            bail!("unreadable certificate");
        }
        let bytes =
            std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data) as usize);
        let fp = sha256::digest(bytes);
        CFRelease(data);
        Ok(fp)
    }
}

pub struct SecureTransport;

impl TlsConnector for SecureTransport {
    fn connect(&self, addr: SocketAddr, pin: Option<&[u8; 32]>) -> Result<TlsConn> {
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
        tcp.set_nodelay(true)?;
        tcp.set_read_timeout(Some(Duration::from_secs(10)))?;
        let handle = tcp.try_clone()?;
        let boxed = Box::new(tcp);

        // SAFETY: standard SecureTransport setup; the connection pointer is
        // the boxed socket, which `TlsStream` keeps alive with the context.
        let ctx = unsafe { SSLCreateContext(ptr::null(), K_SSL_CLIENT_SIDE, K_SSL_STREAM_TYPE) };
        if ctx.is_null() {
            bail!("SSLCreateContext failed");
        }
        let stream = TlsStream { ctx, _tcp: boxed };
        unsafe {
            let conn = &*stream._tcp as *const TcpStream as SSLConnectionRef;
            for (what, status) in [
                ("SSLSetIOFuncs", SSLSetIOFuncs(ctx, read_cb, write_cb)),
                ("SSLSetConnection", SSLSetConnection(ctx, conn)),
                (
                    "SSLSetProtocolVersionMin",
                    SSLSetProtocolVersionMin(ctx, K_TLS_PROTOCOL_12),
                ),
                (
                    "SSLSetSessionOption",
                    SSLSetSessionOption(ctx, K_SSL_SESSION_OPTION_BREAK_ON_SERVER_AUTH, 1),
                ),
            ] {
                if status != 0 {
                    bail!("{what} failed ({status})");
                }
            }
        }

        let mut fingerprint = None;
        loop {
            // SAFETY: valid, configured context.
            match unsafe { SSLHandshake(ctx) } {
                0 => break,
                ERR_SSL_SERVER_AUTH_COMPLETED => {
                    // We do our own validation: pin or record the certificate.
                    let fp = peer_fingerprint(ctx)?;
                    if let Some(pin) = pin
                        && *pin != fp
                    {
                        bail!(
                            "the phone's certificate changed (expected {}, got {}); \
                             forget the pairing and pair again if you reinstalled the app",
                            &sha256::hex(pin)[..16],
                            &sha256::hex(&fp)[..16]
                        );
                    }
                    fingerprint = Some(fp);
                }
                ERR_SSL_WOULD_BLOCK => bail!("TLS handshake timed out"),
                s => bail!("TLS handshake failed ({s})"),
            }
        }
        let Some(fingerprint) = fingerprint else {
            bail!("the phone sent no certificate");
        };
        handle.set_read_timeout(None)?;
        Ok(TlsConn {
            stream: Box::new(stream),
            tcp: handle,
            fingerprint,
        })
    }
}
