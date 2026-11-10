//! Finds phones running the DroidMirror app (`_droidmirror._tcp`) with
//! Bonjour. The browser and its delegate live on the main thread; the list
//! of resolved phones is shared with the session thread.
//!
//! NSNetService is deprecated in favour of Network.framework's nw_browser,
//! which needs Objective-C blocks; NSNetService still works and keeps this
//! within the objc2 crates already in use.
#![allow(deprecated)]

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_foundation::{
    MainThreadMarker, NSDictionary, NSNetService, NSNetServiceBrowser, NSNetServiceBrowserDelegate,
    NSNetServiceDelegate, NSObject, NSObjectProtocol, NSString, ns_string,
};

use crate::companion::{Discovery, Peer, SERVICE_TYPE};

type Peers = Arc<Mutex<Vec<Peer>>>;

/// Thread-safe view of the discovered phones.
pub struct BonjourPeers(Peers);

impl Discovery for BonjourPeers {
    fn peers(&self) -> Vec<Peer> {
        self.0.lock().unwrap().clone()
    }
}

pub struct BrowserIvars {
    peers: Peers,
    /// Services being resolved or resolved; NSNetService must be retained.
    services: RefCell<Vec<Retained<NSNetService>>>,
}

define_class!(
    // SAFETY: NSObject subclass without Drop; delegate methods have the
    // signatures declared by the protocols.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DMBonjourDelegate"]
    #[ivars = BrowserIvars]
    pub struct BrowserDelegate;

    unsafe impl NSObjectProtocol for BrowserDelegate {}

    unsafe impl NSNetServiceBrowserDelegate for BrowserDelegate {
        #[unsafe(method(netServiceBrowser:didFindService:moreComing:))]
        fn did_find(&self, _browser: &NSNetServiceBrowser, service: &NSNetService, _more: bool) {
            log::debug!("bonjour: found {}", service.name());
            let service = service.retain();
            // SAFETY: the delegate (self) outlives the services it retains.
            unsafe { service.setDelegate(Some(ProtocolObject::from_ref(self))) };
            service.resolveWithTimeout(10.0);
            self.ivars().services.borrow_mut().push(service);
        }

        #[unsafe(method(netServiceBrowser:didRemoveService:moreComing:))]
        fn did_remove(&self, _browser: &NSNetServiceBrowser, service: &NSNetService, _more: bool) {
            let name = service.name().to_string();
            log::debug!("bonjour: {name} went away");
            self.ivars()
                .services
                .borrow_mut()
                .retain(|s| s.name().to_string() != name);
            self.ivars()
                .peers
                .lock()
                .unwrap()
                .retain(|p| p.name != name);
        }
    }

    unsafe impl NSNetServiceDelegate for BrowserDelegate {
        #[unsafe(method(netServiceDidResolveAddress:))]
        fn did_resolve(&self, service: &NSNetService) {
            let Some(peer) = peer_from(service) else {
                return;
            };
            log::info!("companion app found: {} at {}", peer.name, peer.addr);
            let mut peers = self.ivars().peers.lock().unwrap();
            peers.retain(|p| p.id != peer.id && p.name != peer.name);
            peers.push(peer);
        }
    }
);

/// Name, id (TXT `id`, falling back to the name) and first IPv4 address.
fn peer_from(service: &NSNetService) -> Option<Peer> {
    let name = service.name().to_string();
    let port = u16::try_from(service.port()).ok()?;
    let ip = service.addresses()?.iter().find_map(|data| {
        let b = data.to_vec();
        // struct sockaddr_in: len, family (AF_INET = 2), port, address.
        (b.len() >= 8 && b[1] == 2).then(|| Ipv4Addr::new(b[4], b[5], b[6], b[7]))
    })?;
    let id = service
        .TXTRecordData()
        .and_then(|txt| {
            let dict: Retained<NSDictionary<NSString, objc2_foundation::NSData>> =
                NSNetService::dictionaryFromTXTRecordData(&txt);
            dict.objectForKey(ns_string!("id"))
        })
        .map(|d| String::from_utf8_lossy(&d.to_vec()).into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| name.clone());
    Some(Peer {
        name,
        id,
        addr: SocketAddr::from((ip, port)),
    })
}

/// Keeps the browser alive; drop it to stop browsing.
pub struct Browser {
    _browser: Retained<NSNetServiceBrowser>,
    _delegate: Retained<BrowserDelegate>,
}

/// Starts browsing on the main run loop.
pub fn start(mtm: MainThreadMarker) -> (Browser, Arc<BonjourPeers>) {
    let peers: Peers = Arc::default();
    let delegate = BrowserDelegate::alloc(mtm).set_ivars(BrowserIvars {
        peers: peers.clone(),
        services: RefCell::new(Vec::new()),
    });
    // SAFETY: NSObject's init.
    let delegate: Retained<BrowserDelegate> = unsafe { msg_send![super(delegate), init] };
    let browser = NSNetServiceBrowser::new();
    // SAFETY: the delegate is retained by `Browser` for the browser's lifetime.
    unsafe { browser.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    browser
        .searchForServicesOfType_inDomain(&NSString::from_str(SERVICE_TYPE), ns_string!("local."));
    (
        Browser {
            _browser: browser,
            _delegate: delegate,
        },
        Arc::new(BonjourPeers(peers)),
    )
}
