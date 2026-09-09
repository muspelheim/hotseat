//! Stable identity for a physical monitor.
//!
//! Backends hand out their own display handles — a list index, an OS display id,
//! a system UUID — and every one of them can change when you move a cable to a
//! different port. Since hotseat's whole job is to survive exactly that, config
//! is keyed on EDID-derived identity instead: manufacturer, model and serial,
//! which travel with the panel rather than with the socket.

use serde::{Deserialize, Serialize};

/// Identity of a physical monitor, derived from its EDID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorId {
    /// Three-character PnP manufacturer id, e.g. `SAM`.
    pub manufacturer: Option<String>,
    /// Numeric product/model id.
    pub model_id: Option<u16>,
    /// Numeric serial from the EDID.
    pub serial: Option<u32>,
    /// Model name string, e.g. `Odyssey G52A`.
    pub model_name: Option<String>,
    /// Alphanumeric serial string, e.g. `H4ZT300577`.
    pub serial_number: Option<String>,
}

impl MonitorId {
    /// Extract identity from a `ddc-hi` display info block.
    pub fn from_info(info: &ddc_hi::DisplayInfo) -> Self {
        Self {
            manufacturer: info.manufacturer_id.clone(),
            model_id: info.model_id,
            serial: info.serial,
            model_name: info.model_name.clone(),
            serial_number: info.serial_number.clone(),
        }
    }

    /// Stable key for config files.
    ///
    /// Built only from EDID fields that travel with the panel, so it is
    /// unchanged by moving the cable between ports or machines. Returns `None`
    /// when the backend supplied no EDID at all — notably possible on Windows,
    /// where WinAPI does not expose it.
    pub fn key(&self) -> Option<String> {
        let manufacturer = self.manufacturer.as_deref();
        let has_any = manufacturer.is_some()
            || self.model_id.is_some()
            || self.serial.is_some()
            || self.serial_number.is_some();
        if !has_any {
            return None;
        }

        // Prefer the alphanumeric serial when present: it is the most specific
        // field and is printed on the panel, so a human can verify it.
        let serial = self
            .serial_number
            .clone()
            .or_else(|| self.serial.map(|s| s.to_string()))
            .unwrap_or_else(|| "noserial".into());

        Some(format!(
            "{}:{}:{}",
            manufacturer.unwrap_or("???"),
            self.model_id
                .map(|m| format!("{m:04x}"))
                .unwrap_or_else(|| "????".into()),
            serial
        ))
    }

    /// Human-facing label for reports.
    pub fn label(&self) -> String {
        match (&self.model_name, &self.manufacturer) {
            (Some(name), Some(vendor)) => format!("{name} ({vendor})"),
            (Some(name), None) => name.clone(),
            (None, Some(vendor)) => format!("unnamed {vendor} display"),
            (None, None) => "unidentified display".into(),
        }
    }

    /// Human-facing label, falling back to a backend-supplied name.
    ///
    /// Needed because `ddc-macos` reports no EDID fields at all: on macOS every
    /// identity field here is `None` and the backend handle is the only place
    /// the product name appears. Using it for display is honest; using it as a
    /// config key would not be, since it is neither unique across two identical
    /// panels nor guaranteed stable — hence [`Self::key`] still returns `None`.
    pub fn label_with_fallback(&self, backend_id: &str) -> String {
        if self.model_name.is_none() && self.manufacturer.is_none() && !backend_id.is_empty() {
            return format!("{backend_id} (name from backend, no EDID)");
        }
        self.label()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference device this project was built against.
    fn g52a() -> MonitorId {
        MonitorId {
            manufacturer: Some("SAM".into()),
            model_id: Some(29056),
            serial: Some(1_129_919_028),
            model_name: Some("Odyssey G52A".into()),
            serial_number: Some("H4ZT300577".into()),
        }
    }

    #[test]
    fn key_prefers_the_human_readable_serial() {
        assert_eq!(g52a().key().as_deref(), Some("SAM:7180:H4ZT300577"));
    }

    #[test]
    fn key_ignores_fields_that_change_with_cabling() {
        // Identity must not depend on model_name, which some backends omit or
        // spell differently, and must never involve a backend handle.
        let mut without_name = g52a();
        without_name.model_name = None;
        assert_eq!(without_name.key(), g52a().key());
    }

    #[test]
    fn key_falls_back_to_the_numeric_serial() {
        let mut id = g52a();
        id.serial_number = None;
        assert_eq!(id.key().as_deref(), Some("SAM:7180:1129919028"));
    }

    #[test]
    fn a_display_with_no_edid_has_no_key() {
        let bare = MonitorId {
            manufacturer: None,
            model_id: None,
            serial: None,
            model_name: None,
            serial_number: None,
        };
        assert_eq!(bare.key(), None);
        assert_eq!(bare.label(), "unidentified display");
    }

    #[test]
    fn labels_degrade_gracefully() {
        assert_eq!(g52a().label(), "Odyssey G52A (SAM)");
        let mut partial = g52a();
        partial.model_name = None;
        assert_eq!(partial.label(), "unnamed SAM display");
    }
}
