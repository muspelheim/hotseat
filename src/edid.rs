//! Just enough EDID parsing to identify a panel.
//!
//! `ddc-hi` attempts this itself but falls back to an empty `DisplayInfo` when
//! either the backend yields no EDID or the parse fails, and it does not say
//! which happened. On macOS that leaves no identity at all, so config has
//! nothing stable to key on. Since `ddc_macos::Monitor::edid()` hands over the
//! raw bytes regardless, hotseat parses the identity block itself.
//!
//! Only the first 18 bytes matter here — the fixed header. Everything else in
//! an EDID (timings, descriptors, extension blocks) is irrelevant to identity
//! and deliberately not touched.

/// The eight-byte fixed pattern that opens every valid EDID.
const MAGIC: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

/// Identity fields from an EDID header block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdidIdentity {
    /// Three-letter PnP manufacturer id, e.g. `SAM`.
    pub manufacturer: String,
    /// Product code.
    pub product_code: u16,
    /// Serial number. Zero means the panel did not supply one.
    pub serial: u32,
    /// Manufacture week, when plausible.
    pub week: Option<u8>,
    /// Manufacture year.
    pub year: Option<u16>,
}

/// Why an EDID could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdidError {
    /// Fewer than 18 bytes, so the identity block is incomplete.
    TooShort(usize),
    /// The eight-byte header pattern is absent.
    BadMagic,
    /// The manufacturer field decoded to something outside A-Z.
    BadManufacturer(u16),
}

impl std::fmt::Display for EdidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EdidError::TooShort(n) => {
                write!(f, "EDID too short: {n} bytes, need at least 18")
            }
            EdidError::BadMagic => write!(f, "EDID header pattern missing"),
            EdidError::BadManufacturer(v) => {
                write!(f, "manufacturer field {v:#06x} does not decode to letters")
            }
        }
    }
}

impl std::error::Error for EdidError {}

/// Decode the manufacturer id: three five-bit letters packed big-endian, with
/// 1 meaning `A`.
fn decode_manufacturer(raw: u16) -> Result<String, EdidError> {
    let letters = [
        ((raw >> 10) & 0x1F) as u8,
        ((raw >> 5) & 0x1F) as u8,
        (raw & 0x1F) as u8,
    ];
    let mut s = String::with_capacity(3);
    for l in letters {
        if !(1..=26).contains(&l) {
            return Err(EdidError::BadManufacturer(raw));
        }
        s.push((b'A' + l - 1) as char);
    }
    Ok(s)
}

/// Parse the identity fields out of an EDID blob.
pub fn parse(bytes: &[u8]) -> Result<EdidIdentity, EdidError> {
    if bytes.len() < 18 {
        return Err(EdidError::TooShort(bytes.len()));
    }
    if bytes[0..8] != MAGIC {
        return Err(EdidError::BadMagic);
    }

    // Manufacturer is big-endian; product code and serial are little-endian.
    let manufacturer = decode_manufacturer(u16::from_be_bytes([bytes[8], bytes[9]]))?;
    let product_code = u16::from_le_bytes([bytes[10], bytes[11]]);
    let serial = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);

    // Week 0 means unspecified, and 0xFF marks the model-year variant where the
    // week field carries no date.
    let week = match bytes[16] {
        0 | 0xFF => None,
        w => Some(w),
    };
    // Year is stored as an offset from 1990. Zero means unspecified.
    let year = match bytes[17] {
        0 => None,
        y => Some(1990 + y as u16),
    };

    Ok(EdidIdentity {
        manufacturer,
        product_code,
        serial,
        week,
        year,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an EDID header with the given identity fields.
    fn header(mfr: u16, product: u16, serial: u32, week: u8, year: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(18);
        v.extend_from_slice(&MAGIC);
        v.extend_from_slice(&mfr.to_be_bytes());
        v.extend_from_slice(&product.to_le_bytes());
        v.extend_from_slice(&serial.to_le_bytes());
        v.push(week);
        v.push(year);
        v
    }

    /// `SAM` packs as S=19, A=1, M=13.
    const SAM: u16 = (19 << 10) | (1 << 5) | 13;

    #[test]
    fn decodes_a_known_manufacturer() {
        assert_eq!(decode_manufacturer(SAM).unwrap(), "SAM");
    }

    #[test]
    fn parses_the_reference_monitor_identity() {
        // Values observed from the Odyssey G52A: model 29056, serial
        // 1129919028, week 14 of 2022.
        let bytes = header(SAM, 29056, 1_129_919_028, 14, 32);
        let id = parse(&bytes).unwrap();
        assert_eq!(id.manufacturer, "SAM");
        assert_eq!(id.product_code, 29056);
        assert_eq!(id.serial, 1_129_919_028);
        assert_eq!(id.week, Some(14));
        assert_eq!(id.year, Some(2022));
    }

    #[test]
    fn endianness_is_not_accidentally_symmetric() {
        // 0x1234 read the wrong way round is 0x3412, so a byte-order mistake
        // cannot pass this.
        let bytes = header(SAM, 0x1234, 0x1122_3344, 1, 30);
        let id = parse(&bytes).unwrap();
        assert_eq!(id.product_code, 0x1234);
        assert_eq!(id.serial, 0x1122_3344);
    }

    #[test]
    fn rejects_a_blob_without_the_header_pattern() {
        let mut bytes = header(SAM, 1, 1, 1, 30);
        bytes[0] = 0x42;
        assert_eq!(parse(&bytes), Err(EdidError::BadMagic));
    }

    #[test]
    fn rejects_a_truncated_blob() {
        assert_eq!(parse(&[0u8; 8]), Err(EdidError::TooShort(8)));
        assert_eq!(parse(&[]), Err(EdidError::TooShort(0)));
    }

    #[test]
    fn rejects_a_manufacturer_outside_the_alphabet() {
        // Letter index 0 is not a letter.
        let bytes = header(0, 1, 1, 1, 30);
        assert!(matches!(parse(&bytes), Err(EdidError::BadManufacturer(_))));
    }

    #[test]
    fn unspecified_dates_become_none_rather_than_1990() {
        let bytes = header(SAM, 1, 1, 0, 0);
        let id = parse(&bytes).unwrap();
        assert_eq!(id.week, None);
        assert_eq!(id.year, None);
    }

    #[test]
    fn the_model_year_week_sentinel_is_not_a_week() {
        let bytes = header(SAM, 1, 1, 0xFF, 32);
        assert_eq!(parse(&bytes).unwrap().week, None);
    }

    #[test]
    fn extra_bytes_beyond_the_header_are_ignored() {
        let mut bytes = header(SAM, 29056, 7, 14, 32);
        bytes.extend_from_slice(&[0xAB; 110]); // rest of a real 128-byte EDID
        assert_eq!(parse(&bytes).unwrap().product_code, 29056);
    }
}
