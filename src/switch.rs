//! Changing a display's input.
//!
//! The one thing to internalise here: **a successful DDC write is not proof the
//! monitor obeyed.** Writes are fire-and-forget over I²C, so `set_input`
//! returning `Ok` means the bus accepted the bytes, nothing more. The reference
//! monitor for this project accepts writes of values it then ignores, and its
//! read-back of `0x60` is in a different encoding from its writes, so it cannot
//! be used to confirm either.
//!
//! Confirmation therefore comes from a human, once, via `hotseat verify`, and is
//! persisted as [`crate::config::MonitorConfig::verified_inputs`].

use anyhow::{Context, Result, bail};

use crate::config::MonitorConfig;
use crate::inputs::{self, INPUT_SELECT};
use crate::vcp::Vcp;

/// Write an input-source value to the display.
///
/// Returns `Ok` when the DDC layer accepted the write. See the module docs: it
/// is not evidence the display switched.
pub fn set_input<V: Vcp>(vcp: &mut V, code: u8) -> Result<()> {
    vcp.set(INPUT_SELECT, code as u16).with_context(|| {
        format!(
            "writing input {} to VCP {INPUT_SELECT:#04x}",
            inputs::label(code)
        )
    })
}

/// A resolved intention to hand the monitor to a particular target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Where the monitor is going.
    pub target: String,
    /// Value to write.
    pub code: u8,
    /// Whether this value has been confirmed to work on this display.
    pub verified: bool,
}

impl Plan {
    /// One-line summary for output.
    pub fn describe(&self) -> String {
        let confidence = if self.verified {
            "verified"
        } else {
            "UNVERIFIED - if nothing happens, this is the value to suspect"
        };
        format!(
            "hand monitor to {} by writing {} ({})",
            self.target,
            inputs::label(self.code),
            confidence
        )
    }
}

/// Work out which value hands the monitor to a named peer.
///
/// Pure, so the resolution rules are testable without hardware.
pub fn plan_give(monitor: &MonitorConfig, peer_name: &str) -> Result<Plan> {
    let Some(peer) = monitor.peer(peer_name) else {
        if monitor.peers.is_empty() {
            bail!(
                "no peers are configured for {}. Add one with:\n    \
                 hotseat peer add {peer_name} --input <value>\n\
                 Run `hotseat probe` to see which values are plausible.",
                monitor.label
            );
        }
        let known: Vec<&str> = monitor.peers.iter().map(|p| p.name.as_str()).collect();
        bail!(
            "unknown peer {peer_name:?} for {}. Configured peers: {}",
            monitor.label,
            known.join(", ")
        );
    };

    // Refuse to hand the monitor to the input this machine is already on: it
    // would look like a no-op failure and send someone hunting for a DDC bug.
    if monitor.own_input == Some(peer.input) {
        bail!(
            "peer {:?} is recorded on input {}, which is the same input this \
             machine occupies. One of the two is wrong - re-run `hotseat verify`.",
            peer.name,
            inputs::label(peer.input)
        );
    }

    Ok(Plan {
        target: peer.name.clone(),
        code: peer.input,
        verified: monitor.verified_inputs.contains(&peer.input),
    })
}

/// Work out which value reclaims the monitor for this machine.
pub fn plan_take(monitor: &MonitorConfig) -> Result<Plan> {
    let Some(code) = monitor.own_input else {
        bail!(
            "this machine's own input on {} is not known yet. Run:\n    hotseat verify",
            monitor.label
        );
    };

    // A recorded `can_pull == false` means the panel ignores input changes from
    // a machine it is not currently displaying, so this cannot work locally.
    // That constraint is the reason the peer mesh exists.
    if monitor.can_pull == Some(false) {
        bail!(
            "{} does not accept input changes from a machine it is not displaying, \
             so this machine cannot reclaim it. Run `hotseat give <this machine>` \
             from whichever machine currently has the monitor, or press the \
             monitor's own input button.",
            monitor.label
        );
    }

    Ok(Plan {
        target: "this machine".into(),
        code,
        verified: monitor.verified_inputs.contains(&code),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::KeySource;
    use crate::vcp::FakeVcp;

    fn configured() -> MonitorConfig {
        let mut m = MonitorConfig::new(
            "serial:1129919028".into(),
            KeySource::Serial,
            "Odyssey G52A".into(),
        );
        m.own_input = Some(17); // HDMI-1
        m.upsert_peer("win-desktop", 15); // DisplayPort-1
        m
    }

    #[test]
    fn set_input_writes_to_the_input_select_feature() {
        let mut vcp = FakeVcp::default();
        set_input(&mut vcp, 15).unwrap();
        assert_eq!(vcp.writes, vec![(INPUT_SELECT, 15)]);
    }

    #[test]
    fn a_write_failure_names_the_input_in_the_error() {
        let mut vcp = FakeVcp::failing_writes("bus error");
        let err = format!("{:#}", set_input(&mut vcp, 15).unwrap_err());
        assert!(err.contains("DisplayPort-1"), "unhelpful error: {err}");
        assert!(err.contains("bus error"), "lost the cause: {err}");
    }

    #[test]
    fn giving_to_a_known_peer_resolves_its_input() {
        let plan = plan_give(&configured(), "win-desktop").unwrap();
        assert_eq!(plan.code, 15);
        assert_eq!(plan.target, "win-desktop");
        assert!(!plan.verified);
    }

    #[test]
    fn peer_names_are_case_insensitive() {
        assert_eq!(plan_give(&configured(), "WIN-DESKTOP").unwrap().code, 15);
    }

    #[test]
    fn a_verified_value_is_reported_as_such() {
        let mut m = configured();
        m.mark_verified(15);
        let plan = plan_give(&m, "win-desktop").unwrap();
        assert!(plan.verified);
        assert!(plan.describe().contains("verified"));
        assert!(!plan.describe().contains("UNVERIFIED"));
    }

    #[test]
    fn an_unverified_plan_says_where_to_look_when_nothing_happens() {
        // Because a DDC write that goes nowhere raises no error, the unverified
        // case has to be loud.
        let text = plan_give(&configured(), "win-desktop").unwrap().describe();
        assert!(text.contains("UNVERIFIED"));
    }

    #[test]
    fn with_no_peers_the_error_says_how_to_add_one() {
        let mut m = configured();
        m.peers.clear();
        let err = plan_give(&m, "win-desktop").unwrap_err().to_string();
        assert!(err.contains("hotseat peer add"), "unactionable: {err}");
    }

    #[test]
    fn an_unknown_peer_error_lists_the_known_ones() {
        let err = plan_give(&configured(), "linux-box")
            .unwrap_err()
            .to_string();
        assert!(err.contains("linux-box"));
        assert!(
            err.contains("win-desktop"),
            "should list alternatives: {err}"
        );
    }

    #[test]
    fn refuses_to_hand_the_monitor_to_our_own_input() {
        // Otherwise this silently looks like a broken DDC write.
        let mut m = configured();
        m.upsert_peer("win-desktop", 17); // same as own_input
        let err = plan_give(&m, "win-desktop").unwrap_err().to_string();
        assert!(err.contains("same input"), "{err}");
    }

    #[test]
    fn taking_uses_our_own_input() {
        let mut m = configured();
        m.can_pull = Some(true);
        assert_eq!(plan_take(&m).unwrap().code, 17);
    }

    #[test]
    fn taking_without_a_known_own_input_says_to_verify() {
        let mut m = configured();
        m.own_input = None;
        let err = plan_take(&m).unwrap_err().to_string();
        assert!(err.contains("hotseat verify"), "{err}");
    }

    #[test]
    fn taking_on_a_push_only_panel_explains_the_constraint() {
        // The reference monitor's behaviour: it will not obey a machine it is
        // not currently displaying. The error must not look like a bug.
        let mut m = configured();
        m.can_pull = Some(false);
        let err = plan_take(&m).unwrap_err().to_string();
        assert!(err.contains("not displaying"), "{err}");
        assert!(err.contains("hotseat give"), "should say what to do: {err}");
    }

    #[test]
    fn taking_is_allowed_while_can_pull_is_still_unknown() {
        // Unknown must not be treated as false, or a fresh install could never
        // measure the capability in the first place.
        let m = configured();
        assert_eq!(m.can_pull, None);
        assert!(plan_take(&m).is_ok());
    }
}
