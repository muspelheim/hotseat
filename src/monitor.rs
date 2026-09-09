//! Stable identity for a physical monitor.
//!
//! Backends hand out their own display handles — a list index, an OS display id,
//! a system UUID — and every one of them can change when you move a cable to a
//! different port. Since hotseat's whole job is to survive exactly that, config
//! is keyed on EDID-derived identity instead: manufacturer, model and serial,
//! which travel with the panel rather than with the socket.

use serde::{Deserialize, Serialize};

/// Where a monitor key came from, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeySource {
    /// Full panel identity: vendor, model and serial. Survives re-cabling and
    /// distinguishes two otherwise identical monitors.
    Edid,
    /// A serial number without vendor or model. Still unique to one panel, and
    /// the best available where a backend reports a serial but no EDID — the
    /// observed situation on macOS, where `ddc_macos::Monitor::edid()` returns
    /// nothing while `serial_number()` succeeds.
    Serial,
    /// A hash of the capabilities string. Works on every platform, including
    /// Windows where WinAPI exposes no EDID at all, but two identical monitors
    /// produce the same key.
    CapabilitiesFingerprint,
}

impl KeySource {
    /// Whether this key can tell two identical panels apart.
    pub fn is_unique_per_panel(self) -> bool {
        matches!(self, KeySource::Edid | KeySource::Serial)
    }

    /// Short description for reports.
    pub fn describe(self) -> &'static str {
        match self {
            KeySource::Edid => "panel identity",
            KeySource::Serial => "serial number only",
            KeySource::CapabilitiesFingerprint => "capabilities fingerprint",
        }
    }
}

/// A monitor key together with how much it can be relied on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorKey {
    /// The key itself, as written to config.
    pub value: String,
    /// Which tier produced it.
    pub source: KeySource,
}

/// FNV-1a, 64-bit.
///
/// Deliberately hand-rolled rather than using `DefaultHasher`: std explicitly
/// reserves the right to change that algorithm between releases, which would
/// silently invalidate every key already written to a user's config. FNV-1a is
/// fixed forever, needs no dependency, and collision resistance is irrelevant
/// here — this is an identifier, not a security primitive.
pub fn fingerprint(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

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

    /// Best serial available, preferring the alphanumeric one because it is
    /// printed on the panel and so a human can verify it.
    fn best_serial(&self) -> Option<String> {
        self.serial_number
            .clone()
            .or_else(|| self.serial.map(|s| s.to_string()))
    }

    /// Full panel-identity key, requiring both vendor and model.
    ///
    /// Built only from fields that travel with the panel, so it is unchanged by
    /// moving the cable between ports or machines.
    ///
    /// Deliberately returns `None` rather than padding missing fields: an
    /// earlier version emitted keys like `???:????:1129919028`, which look like
    /// identity while carrying almost none. Partial data is handled by the
    /// weaker tiers in [`Self::resolve_key`], which say what they are.
    pub fn key(&self) -> Option<String> {
        let manufacturer = self.manufacturer.as_deref()?;
        let model_id = self.model_id?;
        let serial = self.best_serial().unwrap_or_else(|| "noserial".into());
        Some(format!("{manufacturer}:{model_id:04x}:{serial}"))
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

    /// Resolve the best available key, saying which tier it came from.
    ///
    /// Prefers panel identity. Falls back to a fingerprint of the capabilities
    /// string, which is available on every platform — including Windows, whose
    /// WinAPI backend cannot expose EDID at all — at the cost of colliding
    /// between two identical monitors.
    pub fn resolve_key(&self, capabilities: Option<&[u8]>) -> Option<MonitorKey> {
        if let Some(value) = self.key() {
            return Some(MonitorKey {
                value,
                source: KeySource::Edid,
            });
        }
        // A serial with no vendor or model still identifies one physical panel,
        // which is what config actually needs.
        if let Some(serial) = self.best_serial() {
            return Some(MonitorKey {
                value: format!("serial:{serial}"),
                source: KeySource::Serial,
            });
        }
        capabilities
            .filter(|c| !c.is_empty())
            .map(|caps| MonitorKey {
                value: format!("caps:{}", fingerprint(caps)),
                source: KeySource::CapabilitiesFingerprint,
            })
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

    /// A display with no identity at all — the observed macOS case.
    fn anonymous() -> MonitorId {
        MonitorId {
            manufacturer: None,
            model_id: None,
            serial: None,
            model_name: Some("Odyssey G52A".into()),
            serial_number: None,
        }
    }

    #[test]
    fn fingerprint_is_pinned_to_known_values() {
        // These are the published FNV-1a 64 test vectors. If a future edit
        // swaps the algorithm, every key already in a user's config would
        // change meaning, so the values are asserted literally.
        assert_eq!(fingerprint(b""), "cbf29ce484222325");
        assert_eq!(fingerprint(b"a"), "af63dc4c8601ec8c");
        assert_eq!(fingerprint(b"foobar"), "85944171f73967e8");
    }

    #[test]
    fn fingerprint_is_stable_and_input_sensitive() {
        let caps = b"(prot(monitor)type(lcd)model(FALCON)";
        assert_eq!(fingerprint(caps), fingerprint(caps));
        assert_ne!(fingerprint(caps), fingerprint(b"(prot(monitor)type(lcd)"));
    }

    #[test]
    fn edid_identity_beats_a_capabilities_fingerprint() {
        let key = g52a().resolve_key(Some(b"some caps")).unwrap();
        assert_eq!(key.source, KeySource::Edid);
        assert_eq!(key.value, "SAM:7180:H4ZT300577");
        assert!(key.source.is_unique_per_panel());
    }

    #[test]
    fn a_display_without_edid_falls_back_to_the_fingerprint() {
        // This is what makes config possible on macOS and Windows today.
        let key = anonymous().resolve_key(Some(b"(prot(monitor))")).unwrap();
        assert_eq!(key.source, KeySource::CapabilitiesFingerprint);
        assert!(key.value.starts_with("caps:"));
        // And it must admit it cannot tell two identical panels apart.
        assert!(!key.source.is_unique_per_panel());
    }

    #[test]
    fn partial_identity_never_produces_placeholder_junk() {
        // The real macOS case: ddc_macos::edid() returns None while
        // serial_number() succeeds, leaving a serial and nothing else. An
        // earlier version emitted "???:????:1129919028" and called it panel
        // identity. It must now decline to build an Edid-tier key at all.
        let mut id = anonymous();
        id.serial_number = Some("1129919028".into());

        assert_eq!(
            id.key(),
            None,
            "vendor and model are required for an Edid key"
        );

        let resolved = id.resolve_key(Some(b"(prot(monitor))")).unwrap();
        assert_eq!(resolved.source, KeySource::Serial);
        assert_eq!(resolved.value, "serial:1129919028");
        assert!(!resolved.value.contains('?'));
        // A serial still pins one physical panel, so this tier is trustworthy.
        assert!(resolved.source.is_unique_per_panel());
    }

    #[test]
    fn a_serial_outranks_a_capabilities_fingerprint() {
        let mut id = anonymous();
        id.serial = Some(42);
        let resolved = id.resolve_key(Some(b"caps here")).unwrap();
        assert_eq!(resolved.source, KeySource::Serial);
    }

    #[test]
    fn vendor_without_model_is_not_enough_for_an_edid_key() {
        let mut id = g52a();
        id.model_id = None;
        assert_eq!(id.key(), None);
        let mut id = g52a();
        id.manufacturer = None;
        assert_eq!(id.key(), None);
    }

    #[test]
    fn no_edid_and_no_capabilities_means_no_key() {
        assert!(anonymous().resolve_key(None).is_none());
        // An empty capabilities string is not a usable fingerprint either.
        assert!(anonymous().resolve_key(Some(b"")).is_none());
    }
}
