//! Deciding whether a link's DDC *reads* can be believed.
//!
//! DDC writes are fire-and-forget over I²C; reads require the display to reply.
//! Some links carry writes perfectly while answering every read with the same
//! meaningless number. Measured case: an M4 Max driving a Samsung Odyssey G52A
//! over HDMI returned `110` for luminance, contrast, volume *and* input source
//! alike, while writes worked flawlessly.
//!
//! This matters because the obvious way to build an input switcher — read the
//! current input, then toggle — silently does the wrong thing on such a link.
//! hotseat therefore establishes read trust first and never branches on a read
//! it has not earned the right to believe.

use crate::vcp::{Vcp, VcpRead};

/// Semantically unrelated VCP codes, read together to detect a lying link.
///
/// They are chosen to have no plausible reason to agree: brightness, contrast
/// and volume are independent continuous controls, and input source is an
/// enumeration whose valid values are unrelated to any of them.
pub const PROBE_CODES: &[(u8, &str)] = &[
    (0x10, "luminance"),
    (0x12, "contrast"),
    (0x62, "audio volume"),
    (0x60, "input source"),
];

/// The outcome of reading one probe code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSample {
    /// VCP feature code.
    pub code: u8,
    /// Human label for the report.
    pub label: &'static str,
    /// The read result, with the error rendered as text.
    pub result: Result<VcpRead, String>,
}

/// How much a link's reads can be trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadTrust {
    /// Reads returned distinct, plausible values.
    Trusted,
    /// The display answered no reads at all. Writes may still work.
    NoReplies,
    /// Unrelated codes all returned the same value, so reads carry no
    /// information. Writes may still work perfectly.
    Constant {
        /// The value every code returned.
        value: u16,
        /// The codes that agreed.
        codes: Vec<u8>,
    },
    /// Too few codes answered to draw a conclusion either way.
    Inconclusive,
}

impl ReadTrust {
    /// Whether a value read from this link may be used to make a decision.
    pub fn is_trusted(&self) -> bool {
        matches!(self, ReadTrust::Trusted)
    }
}

/// Minimum successful reads before agreement is treated as evidence of a fault
/// rather than coincidence. Two controls legitimately sharing a value (both at
/// 50, say) is unremarkable; three or more is not.
const MIN_SAMPLES_FOR_VERDICT: usize = 3;

/// Read every probe code and record the results without judging them.
pub fn sample<V: Vcp>(vcp: &mut V) -> Vec<ReadSample> {
    PROBE_CODES
        .iter()
        .map(|&(code, label)| ReadSample {
            code,
            label,
            result: vcp.get(code).map_err(|e| e.to_string()),
        })
        .collect()
}

/// Judge a set of samples.
pub fn assess(samples: &[ReadSample]) -> ReadTrust {
    let ok: Vec<(u8, u16)> = samples
        .iter()
        .filter_map(|s| s.result.as_ref().ok().map(|r| (s.code, r.value)))
        .collect();

    if ok.is_empty() {
        return ReadTrust::NoReplies;
    }
    if ok.len() < MIN_SAMPLES_FOR_VERDICT {
        return ReadTrust::Inconclusive;
    }

    let first = ok[0].1;
    if ok.iter().all(|&(_, v)| v == first) {
        return ReadTrust::Constant {
            value: first,
            codes: ok.iter().map(|&(c, _)| c).collect(),
        };
    }

    ReadTrust::Trusted
}

/// Convenience: sample and assess in one step.
pub fn evaluate<V: Vcp>(vcp: &mut V) -> (Vec<ReadSample>, ReadTrust) {
    let samples = sample(vcp);
    let verdict = assess(&samples);
    (samples, verdict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp::FakeVcp;

    fn codes() -> Vec<u8> {
        PROBE_CODES.iter().map(|&(c, _)| c).collect()
    }

    #[test]
    fn constant_reads_are_not_trusted() {
        // The exact G52A-over-HDMI failure: every code answers 110.
        let mut vcp = FakeVcp::constant(&codes(), 110);
        let (_, verdict) = evaluate(&mut vcp);
        assert_eq!(
            verdict,
            ReadTrust::Constant {
                value: 110,
                codes: codes(),
            }
        );
        assert!(!verdict.is_trusted());
    }

    #[test]
    fn distinct_reads_are_trusted() {
        let mut vcp = FakeVcp::from_pairs(&[
            (0x10, 75, 100), // luminance
            (0x12, 50, 100), // contrast
            (0x62, 30, 100), // volume
            (0x60, 17, 0),   // input source = HDMI-1
        ]);
        let (_, verdict) = evaluate(&mut vcp);
        assert_eq!(verdict, ReadTrust::Trusted);
        assert!(verdict.is_trusted());
    }

    #[test]
    fn a_silent_display_reports_no_replies() {
        let mut vcp = FakeVcp::silent();
        let (samples, verdict) = evaluate(&mut vcp);
        assert_eq!(verdict, ReadTrust::NoReplies);
        assert!(samples.iter().all(|s| s.result.is_err()));
    }

    #[test]
    fn two_coincidentally_equal_reads_are_not_condemned() {
        // Brightness and contrast both at 50 is ordinary, not a fault. With only
        // two replies there is not enough evidence either way.
        let mut vcp = FakeVcp::from_pairs(&[(0x10, 50, 100), (0x12, 50, 100)]);
        let (_, verdict) = evaluate(&mut vcp);
        assert_eq!(verdict, ReadTrust::Inconclusive);
    }

    #[test]
    fn three_equal_reads_with_one_dissenter_is_trusted() {
        // Agreement must be unanimous to count as a fault.
        let mut vcp = FakeVcp::from_pairs(&[
            (0x10, 50, 100),
            (0x12, 50, 100),
            (0x62, 50, 100),
            (0x60, 15, 0),
        ]);
        let (_, verdict) = evaluate(&mut vcp);
        assert_eq!(verdict, ReadTrust::Trusted);
    }
}
