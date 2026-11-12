//! Decoder of the modified UTF-8 used by constant pool strings (JVMS 4.4.7), compatible with
//! the reference decoder (including how it treats malformed sequences).

use crate::error::{JavaError, Result};

/// Decode to UTF-16 code units (unpaired surrogates are kept as they are).
pub(crate) fn decode(bytes: &[u8]) -> Result<Vec<u16>> {
    if bytes.is_ascii() {
        return Ok(bytes.iter().map(|b| *b as u16).collect());
    }
    let len = bytes.len();
    let mut out: Vec<u16> = Vec::with_capacity(len);
    let mut i = 0usize;
    while i < len {
        let x = bytes[i] as u32;
        if x & 0x80 == 0 {
            out.push(x as u16);
        } else {
            if i + 1 >= len {
                return Err(JavaError::format("inconsistent modified UTF-8: too short"));
            }
            let y = bytes[i + 1] as u32;
            if x == 0xc0 && y == 0x80 {
                out.push(0);
                i += 1;
            } else if x & 0xE0 == 0xC0 && y & 0xC0 == 0x80 {
                out.push((((x & 0x1f) << 6) + (y & 0x3f)) as u16);
                i += 1;
            } else if i + 2 < len {
                let z = bytes[i + 2] as u32;
                if x & 0xF0 == 0xE0 && y & 0xC0 == 0x80 && z & 0xC0 == 0x80 {
                    // a surrogate pair stored as two 3 byte sequences decodes as two chars
                    out.push((((x & 0xf) << 12) + ((y & 0x3f) << 6) + (z & 0x3f)) as u16);
                    i += 2;
                } else {
                    return Err(JavaError::format(
                        "inconsistent modified UTF-8: unexpected char",
                    ));
                }
            }
            // else: a trailing byte that starts no valid sequence is dropped by the reference
        }
        i += 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_nul() {
        assert_eq!(decode(b"abc").unwrap(), vec![97, 98, 99]);
        assert_eq!(decode(&[0xc0, 0x80]).unwrap(), vec![0]);
    }

    #[test]
    fn multi_byte() {
        // U+00E9, U+20AC
        assert_eq!(
            decode(&[0xc3, 0xa9, 0xe2, 0x82, 0xac]).unwrap(),
            vec![0xe9, 0x20ac]
        );
        // surrogate pair as two 3 byte sequences
        assert_eq!(
            decode(&[0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80]).unwrap(),
            vec![0xd83d, 0xde00]
        );
    }
}
