//! The minimal DDC/CI surface hotseat needs, behind a trait.
//!
//! Everything above this module is written against [`Vcp`] rather than against
//! `ddc_hi` directly, so the discovery logic can be tested against recorded or
//! synthetic monitors in CI where no display exists.

use anyhow::Result;

/// One VCP feature read: the current value and the maximum the display reports.
///
/// `maximum` is worth carrying around even when you only want `value`, because
/// a nonsensical maximum is often the first sign that a link is not really
/// answering reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VcpRead {
    /// Current value of the feature.
    pub value: u16,
    /// Maximum value the display claims for this feature.
    pub maximum: u16,
}

/// Read and write access to a display's VCP features.
pub trait Vcp {
    /// Read a VCP feature.
    fn get(&mut self, code: u8) -> Result<VcpRead>;

    /// Write a VCP feature.
    fn set(&mut self, code: u8, value: u16) -> Result<()>;

    /// Fetch the raw MCCS capabilities string.
    fn capabilities_raw(&mut self) -> Result<Vec<u8>>;
}

/// [`Vcp`] backed by a real display via `ddc-hi`.
pub struct Handle<'a>(&'a mut ddc_hi::Handle);

impl<'a> Handle<'a> {
    /// Borrow a `ddc_hi` handle as a [`Vcp`].
    pub fn new(handle: &'a mut ddc_hi::Handle) -> Self {
        Self(handle)
    }
}

impl Vcp for Handle<'_> {
    fn get(&mut self, code: u8) -> Result<VcpRead> {
        use ddc_hi::Ddc;
        let v = self.0.get_vcp_feature(code)?;
        Ok(VcpRead {
            value: v.value(),
            maximum: v.maximum(),
        })
    }

    fn set(&mut self, code: u8, value: u16) -> Result<()> {
        use ddc_hi::Ddc;
        self.0.set_vcp_feature(code, value)?;
        Ok(())
    }

    fn capabilities_raw(&mut self) -> Result<Vec<u8>> {
        use ddc_hi::Ddc;
        self.0.capabilities_string()
    }
}

/// A scripted [`Vcp`] for tests: answers reads from a lookup table.
#[cfg(test)]
pub struct FakeVcp {
    /// Values returned per code. A missing code produces an error.
    pub reads: std::collections::BTreeMap<u8, VcpRead>,
}

#[cfg(test)]
impl FakeVcp {
    /// Answer every listed code with the same value — the failure mode observed
    /// on an M4 Max talking to a Samsung G52A over HDMI.
    pub fn constant(codes: &[u8], value: u16) -> Self {
        Self {
            reads: codes
                .iter()
                .map(|&c| {
                    (
                        c,
                        VcpRead {
                            value,
                            maximum: value,
                        },
                    )
                })
                .collect(),
        }
    }

    /// Answer each code with its own plausible value.
    pub fn from_pairs(pairs: &[(u8, u16, u16)]) -> Self {
        Self {
            reads: pairs
                .iter()
                .map(|&(c, value, maximum)| (c, VcpRead { value, maximum }))
                .collect(),
        }
    }

    /// A display that never answers reads at all.
    pub fn silent() -> Self {
        Self {
            reads: std::collections::BTreeMap::new(),
        }
    }
}

#[cfg(test)]
impl Vcp for FakeVcp {
    fn get(&mut self, code: u8) -> Result<VcpRead> {
        self.reads
            .get(&code)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("no reply for code {code:#04x}"))
    }

    fn set(&mut self, _code: u8, _value: u16) -> Result<()> {
        Ok(())
    }

    fn capabilities_raw(&mut self) -> Result<Vec<u8>> {
        anyhow::bail!("fake display has no capabilities string")
    }
}
