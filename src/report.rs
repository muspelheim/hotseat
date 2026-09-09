//! Rendering a probe report a human can act on.
//!
//! The report's job is to keep what was *measured* visually distinct from what
//! was *declared* and what was merely *assumed*. A tool that presents a guessed
//! input code with the same confidence as a verified one is how people end up
//! with a hotkey that silently does nothing.

use crate::inputs::{self, Candidate, Provenance};
use crate::monitor::{MonitorId, MonitorKey};
use crate::platform::Recovery;
use crate::trust::{ReadSample, ReadTrust};
use std::fmt::Write as _;

/// Everything M1 can learn about one display without writing to it.
pub struct DisplayReport {
    /// Backend that found the display.
    pub backend: String,
    /// Backend-specific handle. Not stable across re-cabling.
    pub backend_id: String,
    /// EDID-derived identity, captured before the capabilities string could
    /// overwrite it.
    pub id: MonitorId,
    /// Model string from the capabilities report, when it differs from the
    /// product name. Often an internal codename.
    pub declared_model: Option<String>,
    /// Whether raw EDID bytes were available.
    pub has_edid: bool,
    /// What identity recovery managed to add beyond what the backend gave.
    pub recovery: Recovery,
    /// Best available config key, and which tier produced it.
    pub key: Option<MonitorKey>,
    /// Length of the capabilities string, or why it could not be fetched.
    pub capabilities: Result<usize, String>,
    /// Raw read results for the trust probe.
    pub samples: Vec<ReadSample>,
    /// Verdict on whether reads can be believed.
    pub trust: ReadTrust,
    /// Candidate input values, best-believed first.
    pub candidates: Vec<Candidate>,
    /// Values the display declared for `0x60`.
    pub declared_inputs: Vec<u8>,
    /// Current input as read back, when reads are trustworthy.
    pub current_input: Option<u8>,
}

impl DisplayReport {
    /// Render the report as text.
    pub fn render(&self) -> String {
        let mut out = String::new();

        let _ = writeln!(out, "{}", self.id.label_with_fallback(&self.backend_id));
        let _ = writeln!(
            out,
            "  backend        {} (handle {})",
            self.backend, self.backend_id
        );
        if let Some(model) = &self.declared_model {
            let _ = writeln!(
                out,
                "  declared model {model}  <- from the capabilities string, often a codename"
            );
        }
        match &self.key {
            Some(k) if k.source.is_unique_per_panel() => {
                let _ = writeln!(
                    out,
                    "  config key     {}  ({})",
                    k.value,
                    k.source.describe()
                );
            }
            Some(k) => {
                let _ = writeln!(
                    out,
                    "  config key     {}  ({} - two identical monitors\n\
                     \x20                would share this key)",
                    k.value,
                    k.source.describe()
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "  config key     UNAVAILABLE - no panel identity and no capabilities\n\
                     \x20                string, so config has nothing to key on"
                );
            }
        }
        let _ = writeln!(
            out,
            "  edid           {}  |  identity: {}",
            if self.has_edid { "present" } else { "absent" },
            self.recovery
        );
        match &self.capabilities {
            Ok(len) => {
                let _ = writeln!(out, "  capabilities   {len} bytes");
            }
            Err(e) => {
                let _ = writeln!(out, "  capabilities   unavailable ({e})");
            }
        }

        let _ = writeln!(out, "\n  DDC read trust");
        for s in &self.samples {
            match &s.result {
                Ok(r) => {
                    let _ = writeln!(
                        out,
                        "    {:#04x} {:<14} value {:<6} max {}",
                        s.code, s.label, r.value, r.maximum
                    );
                }
                Err(e) => {
                    let _ = writeln!(out, "    {:#04x} {:<14} no reply ({e})", s.code, s.label);
                }
            }
        }
        let _ = writeln!(out, "    verdict: {}", describe_trust(&self.trust));

        let _ = writeln!(out, "\n  input candidates (best-believed first)");
        for c in &self.candidates {
            let _ = writeln!(
                out,
                "    {:#04x} ({:<3}) {:<16} {}",
                c.code,
                c.code,
                c.name,
                c.provenance.tag()
            );
        }
        if !self
            .candidates
            .iter()
            .any(|c| c.provenance == Provenance::Verified)
        {
            let _ = writeln!(
                out,
                "    None of these are verified yet. `declared` means the display said so,\n\
                 \x20   which is only a hint - displays do misreport their own inputs."
            );
        }

        match self.current_input {
            Some(code) => {
                let _ = writeln!(
                    out,
                    "\n  current input  reads back as {}",
                    inputs::label(code)
                );
                if !self.declared_inputs.is_empty() && !self.declared_inputs.contains(&code) {
                    let _ = writeln!(
                        out,
                        "    CAUTION: that value is not among the {} value(s) this display\n\
                         \x20            declares it supports, so its read-back and write\n\
                         \x20            encodings differ. Do not feed this number back as a\n\
                         \x20            write without verifying it first.",
                        self.declared_inputs.len()
                    );
                }
            }
            None => {
                let _ = writeln!(
                    out,
                    "\n  current input  undeterminable - reads are not trustworthy on this link"
                );
            }
        }

        out
    }
}

/// One-line explanation of a trust verdict, written for someone deciding what
/// to do next rather than for a log.
pub fn describe_trust(trust: &ReadTrust) -> String {
    match trust {
        ReadTrust::Trusted => "reads look plausible and may be used for decisions".into(),
        ReadTrust::NoReplies => {
            "the display answered no reads at all; writes may still work".into()
        }
        ReadTrust::Constant { value, codes } => format!(
            "UNTRUSTWORTHY - {} unrelated codes all returned {}. \
             Reads carry no information on this link; writes may still work perfectly.",
            codes.len(),
            value
        ),
        ReadTrust::Inconclusive => "too few replies to judge; treat reads as unreliable".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_constant_link_is_described_as_untrustworthy() {
        let d = describe_trust(&ReadTrust::Constant {
            value: 110,
            codes: vec![0x10, 0x12, 0x62, 0x60],
        });
        assert!(d.contains("UNTRUSTWORTHY"));
        assert!(d.contains("110"));
        // The distinction that matters: reads are dead, writes may not be.
        assert!(d.contains("writes may still work"));
    }

    #[test]
    fn a_trusted_link_is_described_as_usable() {
        assert!(describe_trust(&ReadTrust::Trusted).contains("may be used"));
    }
}
