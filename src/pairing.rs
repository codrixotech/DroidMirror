//! Wi-Fi setup without a cable: QR-code pairing and network discovery,
//! using Android's Wireless debugging (Android 11+) through adb.
//!
//! QR pairing (the same flow Android Studio uses): we show a QR code
//! `WIFI:T:ADB;S:<name>;P:<password>;;`. When the phone scans it from
//! Settings → Developer options → Wireless debugging → "Pair device with QR
//! code", it advertises an mDNS service `_adb-tls-pairing._tcp` whose
//! instance name is `<name>`. We find it with `adb mdns services` and run
//! `adb pair <ip:port> <password>`. After pairing, the phone advertises
//! `_adb-tls-connect._tcp`, which adb auto-connects to (or we connect to it
//! explicitly when the port changes).

use std::hash::{BuildHasher, Hasher, RandomState};

use crate::adb::Adb;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdnsService {
    pub instance: String,
    pub kind: ServiceKind,
    /// `ip:port`
    pub addr: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    /// A paired phone with Wireless debugging on, ready to connect.
    Connect,
    /// A phone waiting to be paired.
    Pairing,
}

/// Parses `adb mdns services` (`instance\tservice\tip:port` per line,
/// from `mdns_list_discovered_services` in adb's transport_mdns.cpp).
pub fn parse_mdns_services(output: &str) -> Vec<MdnsService> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let instance = parts.next()?.trim();
            let service = parts.next()?.trim();
            let addr = parts.next()?.trim();
            let kind = if service.starts_with("_adb-tls-connect") {
                ServiceKind::Connect
            } else if service.starts_with("_adb-tls-pairing") {
                ServiceKind::Pairing
            } else {
                return None;
            };
            // Skip unresolved entries (no IPv4 address yet).
            let (ip, port) = addr.rsplit_once(':')?;
            if ip.is_empty() || ip == "0.0.0.0" || port.parse::<u16>().is_err() {
                return None;
            }
            Some(MdnsService {
                instance: instance.to_owned(),
                kind,
                addr: addr.to_owned(),
            })
        })
        .collect()
}

/// A one-time pairing invitation shown as a QR code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingOffer {
    pub name: String,
    pub password: String,
}

impl PairingOffer {
    pub fn new() -> Self {
        Self {
            name: format!("droidmirror-{}", random_alnum(6)),
            password: random_alnum(10),
        }
    }

    /// Text to encode in the QR code. Only letters, digits and `-` are
    /// used, so nothing needs escaping in the WIFI: format.
    pub fn qr_text(&self) -> String {
        format!("WIFI:T:ADB;S:{};P:{};;", self.name, self.password)
    }

    /// The phone's pairing service for this offer, if it is advertising.
    pub fn find<'a>(&self, services: &'a [MdnsService]) -> Option<&'a MdnsService> {
        services
            .iter()
            .find(|s| s.kind == ServiceKind::Pairing && s.instance == self.name)
    }
}

/// Lowercase letters and digits from a randomly seeded hasher.
fn random_alnum(len: usize) -> String {
    const CHARS: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789"; // no look-alikes
    let state = RandomState::new();
    (0..len)
        .map(|i| {
            let mut h = state.build_hasher();
            h.write_usize(i);
            CHARS[(h.finish() % CHARS.len() as u64) as usize] as char
        })
        .collect()
}

/// `adb mdns services`; empty if mDNS is unavailable.
pub fn discover(adb: &Adb) -> Vec<MdnsService> {
    match adb.mdns_services() {
        Ok(out) => parse_mdns_services(&out),
        Err(e) => {
            log::debug!("mdns discovery unavailable: {e:#}");
            Vec::new()
        }
    }
}

/// Whether `adb pair` output reports success (adb_wifi.cpp).
pub fn pair_succeeded(output: &str) -> bool {
    output.contains("Successfully paired")
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: &str = "List of discovered mdns services\n\
        adb-MRBYXGEMK77PGYDA-iRB5rJ\t_adb-tls-connect._tcp\t192.168.1.23:41235\n\
        droidmirror-ab12cd\t_adb-tls-pairing._tcp.\t192.168.1.23:37099\n\
        adb-OTHER-xyz\t_adb._tcp\t192.168.1.40:5555\n\
        adb-NOIP-1\t_adb-tls-connect._tcp\t0.0.0.0:0\n";

    #[test]
    fn parses_services() {
        assert_eq!(
            parse_mdns_services(OUT),
            vec![
                MdnsService {
                    instance: "adb-MRBYXGEMK77PGYDA-iRB5rJ".into(),
                    kind: ServiceKind::Connect,
                    addr: "192.168.1.23:41235".into(),
                },
                MdnsService {
                    instance: "droidmirror-ab12cd".into(),
                    kind: ServiceKind::Pairing,
                    addr: "192.168.1.23:37099".into(),
                },
            ]
        );
        assert!(parse_mdns_services("List of discovered mdns services\n").is_empty());
    }

    #[test]
    fn finds_our_pairing_service_only() {
        let services = parse_mdns_services(OUT);
        let ours = PairingOffer {
            name: "droidmirror-ab12cd".into(),
            password: "x".into(),
        };
        assert_eq!(ours.find(&services).unwrap().addr, "192.168.1.23:37099");
        let other = PairingOffer {
            name: "droidmirror-zzzzzz".into(),
            password: "x".into(),
        };
        assert!(other.find(&services).is_none());
    }

    #[test]
    fn qr_text_format() {
        let o = PairingOffer {
            name: "droidmirror-ab12cd".into(),
            password: "k3m9p2q8r4".into(),
        };
        assert_eq!(
            o.qr_text(),
            "WIFI:T:ADB;S:droidmirror-ab12cd;P:k3m9p2q8r4;;"
        );
    }

    #[test]
    fn offers_are_random_and_safe() {
        let a = PairingOffer::new();
        let b = PairingOffer::new();
        assert_ne!(a, b);
        assert_eq!(a.password.len(), 10);
        assert!(a.name.starts_with("droidmirror-"));
        assert!(
            a.qr_text()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ":;-".contains(c))
        );
    }

    #[test]
    fn pair_result() {
        assert!(pair_succeeded(
            "Successfully paired to 192.168.1.23:37099 [guid=adb-X-y]\n"
        ));
        assert!(!pair_succeeded(
            "Failed: Wrong password or connection was dropped.\n"
        ));
    }
}
