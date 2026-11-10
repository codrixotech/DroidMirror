//! Protocol tests against a fake phone (plain TCP stands in for TLS).

use super::*;
use std::net::TcpListener;
use std::sync::Mutex;
use std::thread;

const FP: [u8; 32] = [7; 32];
const TOKEN: [u8; 32] = [9; 32];

/// "TLS" that is plain TCP with a fixed certificate fingerprint.
struct PlainTls {
    pins: Mutex<Vec<Option<[u8; 32]>>>,
}

impl TlsConnector for PlainTls {
    fn connect(&self, addr: SocketAddr, pin: Option<&[u8; 32]>) -> Result<TlsConn> {
        self.pins.lock().unwrap().push(pin.copied());
        if pin.is_some_and(|p| *p != FP) {
            bail!("certificate mismatch");
        }
        let tcp = TcpStream::connect(addr)?;
        Ok(TlsConn {
            stream: Box::new(tcp.try_clone()?),
            tcp,
            fingerprint: FP,
        })
    }
}

struct NoDiscovery;
impl Discovery for NoDiscovery {
    fn peers(&self) -> Vec<Peer> {
        Vec::new()
    }
}

fn companion(tag: &str) -> (Companion, Arc<PlainTls>, std::path::PathBuf) {
    let tls = Arc::new(PlainTls {
        pins: Mutex::new(Vec::new()),
    });
    let path = std::env::temp_dir().join(format!("dm-comp-{tag}-{}", std::process::id()));
    let c = Companion {
        tls: tls.clone(),
        discovery: Arc::new(NoDiscovery),
        store: Arc::new(PairingStore::new(path.clone())),
        mac_name: "Test Mac".into(),
    };
    (c, tls, path)
}

/// Fake phone: answers one connection with `reply(hello) -> bytes`.
fn phone(reply: impl FnOnce(Vec<u8>) -> Vec<u8> + Send + 'static) -> Peer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut head = [0u8; 5];
        s.read_exact(&mut head).unwrap();
        assert_eq!(&head[..4], b"DMC1");
        let mut payload = vec![head[4]];
        let rest = if head[4] == Channel::Pair as u8 {
            let mut n = [0u8; 17];
            s.read_exact(&mut n).unwrap();
            let mut name = vec![0u8; n[16] as usize];
            s.read_exact(&mut name).unwrap();
            [n.to_vec(), name].concat()
        } else {
            let mut t = [0u8; 32];
            s.read_exact(&mut t).unwrap();
            t.to_vec()
        };
        payload.extend(rest);
        s.write_all(&reply(payload)).unwrap();
    });
    Peer {
        name: "realme RMX3771".into(),
        id: "phone-1".into(),
        addr,
    }
}

#[test]
fn pairing_succeeds_and_codes_match() {
    let (c, _, path) = companion("pair");
    let phone_code = Arc::new(Mutex::new(None));
    let pc = phone_code.clone();
    let peer = phone(move |hello| {
        assert_eq!(hello[0], Channel::Pair as u8);
        let nonce: [u8; 16] = hello[1..17].try_into().unwrap();
        assert_eq!(&hello[18..], b"Test Mac");
        *pc.lock().unwrap() = Some(format_code(pairing_code(&FP, &nonce)));
        let mut r = vec![0];
        r.extend_from_slice(&TOKEN);
        r.push(7);
        r.extend_from_slice(b"phone-1");
        r
    });
    let shown = Mutex::new(String::new());
    let record = pair(
        &c,
        &peer,
        &|code| *shown.lock().unwrap() = code.to_owned(),
        &|_| {},
    )
    .unwrap();
    assert_eq!(record.token, TOKEN);
    assert_eq!(record.fingerprint, FP);
    // The Mac showed exactly what the phone computed.
    assert_eq!(
        Some(shown.lock().unwrap().clone()),
        *phone_code.lock().unwrap()
    );
    assert_eq!(c.store.get("phone-1"), Some(record));
    let _ = std::fs::remove_file(path);
}

#[test]
fn pairing_declined() {
    let (c, _, _) = companion("deny");
    let peer = phone(|_| vec![1]);
    let err = pair(&c, &peer, &|_| {}, &|_| {}).unwrap_err();
    assert_eq!(
        err.downcast_ref::<CompanionError>(),
        Some(&CompanionError::Denied)
    );
    assert!(c.store.all().is_empty());
}

#[test]
fn open_channel_uses_pin_and_token() {
    let (c, tls, _) = companion("open");
    let peer = phone(|hello| {
        assert_eq!(hello[0], Channel::Video as u8);
        assert_eq!(&hello[1..], &TOKEN);
        vec![0, b'x']
    });
    let record = PairingRecord {
        id: "phone-1".into(),
        name: "p".into(),
        fingerprint: FP,
        token: TOKEN,
    };
    let mut conn = open(&c, &peer, &record, Channel::Video).unwrap();
    let mut b = [0u8; 1];
    conn.stream.read_exact(&mut b).unwrap();
    assert_eq!(b, *b"x");
    assert_eq!(tls.pins.lock().unwrap().as_slice(), &[Some(FP)]);
}

#[test]
fn open_channel_errors() {
    let (c, _, _) = companion("errs");
    let record = PairingRecord {
        id: "phone-1".into(),
        name: "p".into(),
        fingerprint: FP,
        token: TOKEN,
    };
    for (status, expected) in [
        (2, CompanionError::UnknownToken),
        (3, CompanionError::NotSharing),
    ] {
        let peer = phone(move |_| vec![status]);
        let err = open(&c, &peer, &record, Channel::Control).unwrap_err();
        assert_eq!(err.downcast_ref::<CompanionError>(), Some(&expected));
    }
    // Wrong pinned certificate.
    let peer = phone(|_| vec![0]);
    let wrong = PairingRecord {
        fingerprint: [1; 32],
        ..record
    };
    assert!(open(&c, &peer, &wrong, Channel::Video).is_err());
}

#[test]
fn code_format() {
    assert_eq!(format_code(42), "000 042");
    assert_eq!(format_code(123456), "123 456");
    let n: [u8; 16] = random_bytes().unwrap();
    assert!(pairing_code(&FP, &n) < 1_000_000);
}

#[test]
fn cross_language_vector() {
    // Same vector is asserted in the Android app's ProtocolTest.
    assert_eq!(pairing_code(&[7; 32], &[1; 16]), CROSS_VECTOR);
}
const CROSS_VECTOR: u32 = 453614;
