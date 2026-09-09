//! Discovering which input-source values a display actually accepts.
//!
//! VCP feature `0x60` selects the input. MCCS defines standard values, and a
//! display can declare which ones it supports in its capabilities string.
//!
//! **A display's declaration cannot be trusted.** The reference device for this
//! project, a Samsung Odyssey G52A, declares `60(01 03)` — VGA-1 and DVI-1 — on
//! a panel whose only physical inputs are one HDMI and one DisplayPort.
//!
//! Its actual, measured write values are `15` for DisplayPort-1 (the MCCS
//! value) and `5` for HDMI-1 (a vendor value that MCCS calls Composite-1).
//! MCCS `17`, the standard HDMI-1 code, does nothing at all on it. Neither
//! working value appears in its own declaration, and one of them is not the
//! standard code for the input it selects — so a tool that trusted either the
//! declaration or the standard alone would fail. See
//! `tests/fixtures/odyssey-g52a.caps`.
//!
//! So hotseat never *narrows* to what a display claims. It offers the union of
//! everything plausible, labelled by how much each value is worth believing,
//! and treats empirical verification as the only real authority.

/// VCP feature code for input source selection.
pub const INPUT_SELECT: u8 = 0x60;

/// MCCS-standard values for [`INPUT_SELECT`].
pub const STANDARD_INPUTS: &[(u8, &str)] = &[
    (0x01, "VGA-1"),
    (0x02, "VGA-2"),
    (0x03, "DVI-1"),
    (0x04, "DVI-2"),
    (0x05, "Composite-1"),
    (0x06, "Composite-2"),
    (0x07, "S-Video-1"),
    (0x08, "S-Video-2"),
    (0x09, "Tuner-1"),
    (0x0A, "Tuner-2"),
    (0x0B, "Tuner-3"),
    (0x0C, "Component-1"),
    (0x0D, "Component-2"),
    (0x0E, "Component-3"),
    (0x0F, "DisplayPort-1"),
    (0x10, "DisplayPort-2"),
    (0x11, "HDMI-1"),
    (0x12, "HDMI-2"),
];

/// Values outside MCCS that vendors use widely enough to be worth naming.
pub const VENDOR_INPUTS: &[(u8, &str)] = &[(0x1B, "USB-C")];

/// How much a candidate input value is worth believing, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provenance {
    /// Confirmed by actually switching the display. The only real authority.
    Verified,
    /// The display listed this value in its capabilities string.
    ///
    /// Suggestive, not authoritative: displays demonstrably lie here.
    Declared,
    /// From the MCCS standard table — a well-founded assumption, nothing more.
    Standard,
}

impl Provenance {
    /// Short word for reports.
    pub fn tag(self) -> &'static str {
        match self {
            Provenance::Verified => "verified",
            Provenance::Declared => "declared",
            Provenance::Standard => "standard",
        }
    }
}

/// One input the display may be switchable to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Value to write to [`INPUT_SELECT`].
    pub code: u8,
    /// Human-readable name.
    pub name: String,
    /// How much this value is worth believing.
    pub provenance: Provenance,
}

/// Human-readable name for a known input value.
pub fn name_for(code: u8) -> Option<&'static str> {
    STANDARD_INPUTS
        .iter()
        .chain(VENDOR_INPUTS)
        .find(|&&(c, _)| c == code)
        .map(|&(_, n)| n)
}

/// Label a value, naming it when known and falling back to hex.
pub fn label(code: u8) -> String {
    match name_for(code) {
        Some(name) => format!("{name} ({code}/{code:#04x})"),
        None => format!("unknown ({code}/{code:#04x})"),
    }
}

/// The values a display declares for `0x60`, and the names it gives them.
///
/// Empty when the display declares nothing usable.
pub fn declared(db: &mccs_db::Database) -> Vec<(u8, Option<String>)> {
    if let Some(desc) = db.get(INPUT_SELECT)
        && let mccs_db::ValueType::NonContinuous { values, .. } = &desc.ty
    {
        return values.iter().map(|(&c, n)| (c, n.clone())).collect();
    }
    Vec::new()
}

/// Enumerate every input value worth offering, best-believed first.
///
/// `verified` lists values previously confirmed to work on this display, which
/// M2 will persist in config. Passing an empty slice is correct before anything
/// has been verified.
///
/// The result is a *union*: the MCCS standard table, plus anything the display
/// declared, plus anything already verified. It never omits a plausible value
/// merely because the display failed to mention it — the reference monitor
/// proves that omission would discard the only values that work.
pub fn candidates(db: &mccs_db::Database, verified: &[u8]) -> Vec<Candidate> {
    use std::collections::BTreeMap;

    let mut by_code: BTreeMap<u8, Candidate> = BTreeMap::new();

    // Weakest tier first; stronger tiers overwrite.
    for &(code, name) in STANDARD_INPUTS.iter().chain(VENDOR_INPUTS) {
        by_code.insert(
            code,
            Candidate {
                code,
                name: name.to_owned(),
                provenance: Provenance::Standard,
            },
        );
    }

    for (code, reported) in declared(db) {
        let name = reported
            .or_else(|| name_for(code).map(str::to_owned))
            .unwrap_or_else(|| format!("{code:#04x}"));
        by_code.insert(
            code,
            Candidate {
                code,
                name,
                provenance: Provenance::Declared,
            },
        );
    }

    for &code in verified {
        let entry = by_code.entry(code).or_insert_with(|| Candidate {
            code,
            name: name_for(code).unwrap_or("unknown").to_owned(),
            provenance: Provenance::Verified,
        });
        entry.provenance = Provenance::Verified;
    }

    let mut out: Vec<Candidate> = by_code.into_values().collect();
    // Best-believed first, then by code so output is stable.
    out.sort_by_key(|c| (c.provenance, c.code));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real capabilities string from the reference monitor.
    const G52A_CAPS: &str = include_str!("../tests/fixtures/odyssey-g52a.caps");

    /// Build a database the way production does: parse a real capabilities
    /// string, then apply it. `Database::entries` is private and there is no
    /// direct insert, so this exercises the same path the binary takes.
    fn db_from_caps(caps: &str) -> mccs_db::Database {
        let parsed =
            mccs_caps::parse_capabilities(caps.trim().as_bytes()).expect("fixture must parse");
        let mut db = mccs_db::Database::default();
        db.apply_capabilities(&parsed);
        db
    }

    #[test]
    fn known_codes_are_named() {
        assert_eq!(name_for(0x0F), Some("DisplayPort-1"));
        assert_eq!(name_for(0x11), Some("HDMI-1"));
        assert_eq!(name_for(0x1B), Some("USB-C"));
        assert_eq!(name_for(0xAB), None);
    }

    #[test]
    fn labels_stay_readable_for_unknown_codes() {
        assert_eq!(label(0x0F), "DisplayPort-1 (15/0x0f)");
        assert_eq!(label(0xAB), "unknown (171/0xab)");
    }

    #[test]
    fn the_reference_monitor_declares_only_vga_and_dvi() {
        // Documents the misreport itself, so a future mccs-caps change that
        // altered parsing would be caught here rather than downstream.
        let db = db_from_caps(G52A_CAPS);
        let mut got: Vec<u8> = declared(&db).into_iter().map(|(c, _)| c).collect();
        got.sort_unstable();
        assert_eq!(got, vec![0x01, 0x03]);
    }

    #[test]
    fn capabilities_are_a_signal_not_the_truth() {
        // THE regression test for this whole module. The G52A declares only
        // 01 and 03, while the values that actually switch it are 15
        // (DisplayPort-1) and 5 (HDMI-1, a vendor code MCCS calls
        // Composite-1). Both survive only because candidates are a union of
        // the standard table with whatever was declared; narrowing to the
        // declaration would discard both.
        let db = db_from_caps(G52A_CAPS);
        let found = candidates(&db, &[]);

        let codes: Vec<u8> = found.iter().map(|c| c.code).collect();
        assert!(
            codes.contains(&0x0F),
            "DisplayPort-1 (15) must survive a display that fails to declare it"
        );
        assert!(
            codes.contains(&0x11),
            "HDMI-1 (17) must survive a display that fails to declare it"
        );
        // The value that genuinely selects HDMI on the reference panel. It is
        // only present because the standard table happens to include 5 under a
        // different name, which is precisely why the union matters.
        assert!(
            codes.contains(&0x05),
            "vendor HDMI code 5 must survive; it is what actually works"
        );

        // What it declared is still surfaced, just labelled as declared.
        let declared_codes: Vec<u8> = found
            .iter()
            .filter(|c| c.provenance == Provenance::Declared)
            .map(|c| c.code)
            .collect();
        assert_eq!(declared_codes, vec![0x01, 0x03]);
    }

    #[test]
    fn verified_values_outrank_everything_and_sort_first() {
        let db = db_from_caps(G52A_CAPS);
        let found = candidates(&db, &[0x0F, 0x11]);

        assert_eq!(found[0].code, 0x0F);
        assert_eq!(found[0].provenance, Provenance::Verified);
        assert_eq!(found[1].code, 0x11);
        assert_eq!(found[1].provenance, Provenance::Verified);

        // Verification must beat a declaration, not merely coexist with it.
        let dp = found.iter().find(|c| c.code == 0x0F).unwrap();
        assert_eq!(dp.provenance, Provenance::Verified);
    }

    #[test]
    fn verification_wins_even_over_a_declared_value() {
        let db = db_from_caps(G52A_CAPS);
        // 0x01 is declared by the display; verifying it should promote it.
        let found = candidates(&db, &[0x01]);
        let one = found.iter().find(|c| c.code == 0x01).unwrap();
        assert_eq!(one.provenance, Provenance::Verified);
    }

    #[test]
    fn an_empty_database_still_offers_the_standard_table() {
        let db = mccs_db::Database::default();
        let found = candidates(&db, &[]);
        assert!(declared(&db).is_empty());
        assert!(found.iter().any(|c| c.code == 0x0F));
        assert!(found.iter().any(|c| c.code == 0x11));
        assert!(found.iter().all(|c| c.provenance == Provenance::Standard));
    }

    #[test]
    fn a_verified_code_outside_every_table_is_still_offered() {
        let db = mccs_db::Database::default();
        let found = candidates(&db, &[0x99]);
        let odd = found.iter().find(|c| c.code == 0x99).unwrap();
        assert_eq!(odd.provenance, Provenance::Verified);
    }
}
