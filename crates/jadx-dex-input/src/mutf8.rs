//! Modified UTF-8 (MUTF-8) decoding as used by DEX string data.
//!
//! DEX strings are UTF-16 code unit sequences stored in MUTF-8: NUL is encoded as the two
//! bytes `C0 80` and supplementary characters as surrogate pairs of two 3-byte sequences.
//! The decoder yields UTF-16 code units so unpaired surrogates survive exactly.

use crate::error::{DexError, Result};
use crate::reader::Reader;

/// Decode one `string_data_item` (`uleb128` utf16 length followed by MUTF-8 bytes and a NUL).
pub fn decode_utf16(r: &mut Reader<'_>) -> Result<Vec<u16>> {
    let len = r.uleb128()? as usize;
    // The declared length is attacker controlled: do not pre-allocate more than the data holds.
    let mut out: Vec<u16> = Vec::with_capacity(len.min(1 << 16));
    loop {
        let a = r.u8()? as u16;
        if a == 0 {
            return Ok(out);
        }
        if out.len() >= len {
            return Err(DexError::BadMutf8(
                "string longer than declared utf16 length",
            ));
        }
        if a < 0x80 {
            out.push(a);
        } else if a & 0xE0 == 0xC0 {
            let b = r.u8()? as u16;
            if b & 0xC0 != 0x80 {
                return Err(DexError::BadMutf8("bad second byte"));
            }
            out.push(((a & 0x1F) << 6) | (b & 0x3F));
        } else if a & 0xF0 == 0xE0 {
            let b = r.u8()? as u16;
            let c = r.u8()? as u16;
            if b & 0xC0 != 0x80 || c & 0xC0 != 0x80 {
                return Err(DexError::BadMutf8("bad second or third byte"));
            }
            out.push(((a & 0x0F) << 12) | ((b & 0x3F) << 6) | (c & 0x3F));
        } else {
            return Err(DexError::BadMutf8("bad lead byte"));
        }
    }
}

/// Convert UTF-16 code units to a Rust string. Unpaired surrogates (legal in DEX, not in
/// Rust strings) become U+FFFD; use the code-unit form when exactness matters.
pub fn utf16_to_string(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(bytes: &[u8]) -> Result<Vec<u16>> {
        decode_utf16(&mut Reader::new(bytes, 0))
    }

    #[test]
    fn ascii() {
        assert_eq!(
            utf16_to_string(&dec(&[3, b'a', b'b', b'c', 0]).unwrap()),
            "abc"
        );
    }

    #[test]
    fn embedded_nul_is_c0_80() {
        assert_eq!(dec(&[1, 0xC0, 0x80, 0]).unwrap(), vec![0u16]);
    }

    #[test]
    fn two_and_three_byte() {
        // U+00E9, U+20AC
        assert_eq!(
            dec(&[2, 0xC3, 0xA9, 0xE2, 0x82, 0xAC, 0]).unwrap(),
            vec![0xE9, 0x20AC]
        );
    }

    #[test]
    fn surrogate_pair_is_two_three_byte_sequences() {
        // U+1F600 = D83D DE00, each encoded as 3 bytes
        let s = dec(&[2, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80, 0]).unwrap();
        assert_eq!(s, vec![0xD83D, 0xDE00]);
        assert_eq!(utf16_to_string(&s), "\u{1F600}");
    }

    #[test]
    fn lone_surrogate_is_preserved_as_units() {
        let s = dec(&[1, 0xED, 0xA0, 0xBD, 0]).unwrap();
        assert_eq!(s, vec![0xD83D]);
        assert_eq!(utf16_to_string(&s), "\u{FFFD}");
    }

    #[test]
    fn errors() {
        assert!(dec(&[1, 0xC3, 0x20, 0]).is_err());
        assert!(dec(&[1, 0xF0, 0x80, 0x80, 0x80, 0]).is_err());
        assert!(dec(&[1, b'a', b'b', 0]).is_err());
        assert!(dec(&[5, b'a']).is_err());
    }
}
