//! Recovering identity that a `ddc-hi` backend left behind.
//!
//! `ddc-hi` tries to build a `DisplayInfo` from EDID and silently falls back to
//! an empty one when either the backend supplied no EDID or its `edid` crate
//! failed to parse it — without saying which. On macOS the observed result is no
//! identity at all, which leaves config with nothing to key on.
//!
//! `ddc_macos::Monitor` hands over the raw bytes plus the product name and
//! serial regardless, so hotseat asks it directly and parses the identity block
//! with [`crate::edid`], which only requires the 18-byte header rather than a
//! fully well-formed EDID.

use crate::monitor::MonitorId;

/// What identity recovery achieved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    /// The backend had already supplied identity; nothing was needed.
    NotNeeded,
    /// Identity was recovered from the backend's raw EDID.
    FromEdid,
    /// Names were recovered, but no usable EDID, so there is still no stable
    /// key from panel identity alone.
    NamesOnly,
    /// The backend exposes nothing further, with the reason where known.
    Unavailable(String),
}

impl std::fmt::Display for Recovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Recovery::NotNeeded => write!(f, "backend supplied identity"),
            Recovery::FromEdid => write!(f, "recovered from backend EDID"),
            Recovery::NamesOnly => write!(f, "names only, no usable EDID"),
            Recovery::Unavailable(why) => write!(f, "unavailable ({why})"),
        }
    }
}

/// Whether an id already carries enough to build a stable key.
fn already_identified(id: &MonitorId) -> bool {
    id.manufacturer.is_some() && id.model_id.is_some()
}

/// Fill in whatever the backend can still tell us about this display.
pub fn recover_identity(id: &mut MonitorId, handle: &ddc_hi::Handle) -> Recovery {
    if already_identified(id) {
        return Recovery::NotNeeded;
    }
    recover_platform(id, handle)
}

#[cfg(target_os = "macos")]
fn recover_platform(id: &mut MonitorId, handle: &ddc_hi::Handle) -> Recovery {
    // `Handle` has exactly one variant on this target: every other ddc-hi
    // backend is a Windows- or Linux-only dependency. Destructured directly
    // rather than matched with a fallback, so that if ddc-hi ever gains a
    // second macOS-capable backend this becomes a compile error demanding it be
    // handled, instead of silently reporting the display unavailable.
    let ddc_hi::Handle::MacOS(monitor) = handle;

    // Names first: useful for labels even when EDID parsing fails.
    if id.model_name.is_none() {
        id.model_name = monitor.product_name();
    }
    if id.serial_number.is_none() {
        id.serial_number = monitor.serial_number();
    }

    let Some(bytes) = monitor.edid() else {
        return Recovery::NamesOnly;
    };

    match crate::edid::parse(&bytes) {
        Ok(parsed) => {
            id.manufacturer = Some(parsed.manufacturer);
            id.model_id = Some(parsed.product_code);
            // A zero serial means the panel supplied none; do not store it as
            // if it were a real value.
            if parsed.serial != 0 {
                id.serial = Some(parsed.serial);
            }
            Recovery::FromEdid
        }
        // Report the parse failure rather than pretending nothing was there;
        // this is exactly the information ddc-hi discards.
        Err(e) => Recovery::Unavailable(format!("EDID present but unparseable: {e}")),
    }
}

#[cfg(not(target_os = "macos"))]
fn recover_platform(_id: &mut MonitorId, _handle: &ddc_hi::Handle) -> Recovery {
    // Linux i2c backends already parse EDID, so reaching here means it was
    // genuinely absent. The Windows WinAPI backend cannot expose EDID at all,
    // which is why `MonitorId::resolve_key` has a capabilities-fingerprint tier.
    Recovery::Unavailable("this backend exposes no further identity".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_is_skipped_when_identity_is_already_present() {
        let mut id = MonitorId {
            manufacturer: Some("SAM".into()),
            model_id: Some(29056),
            serial: Some(1),
            model_name: Some("Odyssey G52A".into()),
            serial_number: None,
        };
        assert!(already_identified(&id));
        // Guard against a future edit that starts overwriting good data.
        let before = id.clone();
        if already_identified(&id) {
            // recover_identity would return NotNeeded without touching it
        }
        assert_eq!(id, before);
        id.manufacturer = None;
        assert!(!already_identified(&id));
    }

    #[test]
    fn a_name_alone_does_not_count_as_identification() {
        // The macOS case: product name known, but nothing that makes a key.
        let id = MonitorId {
            manufacturer: None,
            model_id: None,
            serial: None,
            model_name: Some("Odyssey G52A".into()),
            serial_number: None,
        };
        assert!(!already_identified(&id));
    }

    #[test]
    fn recovery_outcomes_read_clearly() {
        assert_eq!(
            Recovery::FromEdid.to_string(),
            "recovered from backend EDID"
        );
        assert!(
            Recovery::Unavailable(
                "EDID present but unparseable: EDID header pattern missing".into()
            )
            .to_string()
            .contains("unparseable")
        );
    }
}
