//! Incoming-call detection.
//!
//! scrcpy-server doesn't report telephony state, so while connected a
//! background thread runs `dumpsys telephony.registry` once a second and
//! reports changes. Answering and declining are done with the Android call
//! keys through the normal control channel (`KEYCODE_CALL` accepts a ringing
//! call, `KEYCODE_ENDCALL` rejects or ends it).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::adb::Adb;
use crate::session::{Ui, UiEvent};

/// Android `KEYCODE_CALL` / `KEYCODE_ENDCALL`.
pub const KEYCODE_CALL: u32 = 5;
pub const KEYCODE_ENDCALL: u32 = 6;

const POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallState {
    Idle,
    /// Incoming call. The number is often hidden by recent Android versions.
    Ringing {
        number: Option<String>,
    },
    /// A call is in progress (answered or outgoing).
    Active,
}

/// Parses `dumpsys telephony.registry`. `mCallState` is printed once per SIM
/// (`TelephonyManager.CALL_STATE_*`: 0 idle, 1 ringing, 2 off-hook).
/// Returns `None` if the output has no call state at all.
pub fn parse_call_state(dump: &str) -> Option<CallState> {
    let mut found = false;
    let mut ringing = false;
    let mut active = false;
    let mut number = None;
    for line in dump.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("mCallState=") {
            found = true;
            match v.trim() {
                "1" => ringing = true,
                "2" => active = true,
                _ => {}
            }
        } else if let Some(v) = line.strip_prefix("mCallIncomingNumber=") {
            let v = v.trim();
            // Redacted numbers are printed as "***" or similar.
            if number.is_none() && v.chars().any(|c| c.is_ascii_digit()) && !v.contains('*') {
                number = Some(v.to_owned());
            }
        }
    }
    if !found {
        return None;
    }
    Some(if ringing {
        CallState::Ringing { number }
    } else if active {
        CallState::Active
    } else {
        CallState::Idle
    })
}

/// Watches one connection's device until `alive` is cleared.
pub fn spawn_watcher(adb: Adb, serial: String, ui: Arc<dyn Ui>, alive: Arc<AtomicBool>) {
    let spawned = thread::Builder::new()
        .name("call-watch".into())
        .spawn(move || {
            let mut last = CallState::Idle;
            let mut failures = 0;
            while alive.load(Ordering::Relaxed) {
                match adb.shell_output(&serial, &["dumpsys", "telephony.registry"]) {
                    Ok(out) => match parse_call_state(&out) {
                        Some(state) => {
                            failures = 0;
                            if state != last && alive.load(Ordering::Relaxed) {
                                log::info!("call state: {state:?}");
                                ui.post(UiEvent::Call(state.clone()));
                                last = state;
                            }
                        }
                        None => {
                            log::warn!("this phone doesn't report call state; call alerts are off");
                            return;
                        }
                    },
                    Err(e) => {
                        failures += 1;
                        if failures == 3 {
                            log::warn!("call state check failing: {e:#}");
                        }
                    }
                }
                thread::sleep(POLL_INTERVAL);
            }
            if last != CallState::Idle {
                ui.post(UiEvent::Call(CallState::Idle));
            }
        });
    if let Err(e) = spawned {
        log::warn!("could not start call watcher: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(states: &[(u8, &str)]) -> String {
        let mut s = String::from("last known state:\n");
        for (i, (state, number)) in states.iter().enumerate() {
            s += &format!(
                "  Phone Id={i}\n    mCallState={state}\n    mRingingCallState=0\n    \
                 mCallIncomingNumber={number}\n    mServiceState=...\n"
            );
        }
        s
    }

    #[test]
    fn idle() {
        assert_eq!(parse_call_state(&dump(&[(0, "")])), Some(CallState::Idle));
    }

    #[test]
    fn ringing_with_and_without_number() {
        assert_eq!(
            parse_call_state(&dump(&[(1, "+911234567890")])),
            Some(CallState::Ringing {
                number: Some("+911234567890".into())
            })
        );
        assert_eq!(
            parse_call_state(&dump(&[(1, "***")])),
            Some(CallState::Ringing { number: None })
        );
        assert_eq!(
            parse_call_state(&dump(&[(1, "")])),
            Some(CallState::Ringing { number: None })
        );
    }

    #[test]
    fn active() {
        assert_eq!(parse_call_state(&dump(&[(2, "")])), Some(CallState::Active));
    }

    #[test]
    fn dual_sim_any_slot_counts() {
        assert_eq!(
            parse_call_state(&dump(&[(0, ""), (1, "5551234")])),
            Some(CallState::Ringing {
                number: Some("5551234".into())
            })
        );
        // Ringing on one SIM while a call is active on the other: ringing wins.
        assert!(matches!(
            parse_call_state(&dump(&[(2, ""), (1, "")])),
            Some(CallState::Ringing { .. })
        ));
        assert_eq!(
            parse_call_state(&dump(&[(0, ""), (2, "")])),
            Some(CallState::Active)
        );
    }

    #[test]
    fn no_call_state_means_unsupported() {
        assert_eq!(
            parse_call_state("Can't find service: telephony.registry\n"),
            None
        );
        assert_eq!(parse_call_state(""), None);
    }
}
