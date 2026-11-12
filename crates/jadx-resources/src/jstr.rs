//! Java compatible string decoding.

const REPL: u16 = 0xFFFD;

fn not_continuation(b: i32) -> bool {
    (b & 0xc0) != 0x80
}

fn is_malformed3(b1: i32, b2: i32, b3: i32) -> bool {
    (b1 == 0xe0u8 as i8 as i32 && (b2 & 0xe0) == 0x80) || (b2 & 0xc0) != 0x80 || (b3 & 0xc0) != 0x80
}

fn is_malformed3_2(b1: i32, b2: i32) -> bool {
    (b1 == 0xe0u8 as i8 as i32 && (b2 & 0xe0) == 0x80) || (b2 & 0xc0) != 0x80
}

fn is_malformed4(b2: i32, b3: i32, b4: i32) -> bool {
    (b2 & 0xc0) != 0x80 || (b3 & 0xc0) != 0x80 || (b4 & 0xc0) != 0x80
}

fn is_malformed4_2(b1: i32, b2: i32) -> bool {
    (b1 == 0xf0 && !(0x90..=0xbf).contains(&b2))
        || (b1 == 0xf4 && (b2 & 0xf0) != 0x80)
        || (b2 & 0xc0) != 0x80
}

/// Number of bytes after the first one that belong to a malformed 3 byte sequence (1 or 2).
fn malformed3(src: &[i8], sp: usize) -> usize {
    let b1 = i32::from(src[sp]);
    let b2 = i32::from(src[sp + 1]);
    if (b1 == 0xe0u8 as i8 as i32 && (b2 & 0xe0) == 0x80) || not_continuation(b2) {
        1
    } else {
        2
    }
}

fn malformed4(src: &[i8], sp: usize) -> usize {
    let b1 = i32::from(src[sp]) & 0xff;
    let b2 = i32::from(src[sp + 1]) & 0xff;
    if b1 > 0xf4
        || (b1 == 0xf0 && !(0x90..=0xbf).contains(&b2))
        || (b1 == 0xf4 && (b2 & 0xf0) != 0x80)
        || not_continuation(b2)
    {
        return 1;
    }
    if not_continuation(i32::from(src[sp + 2])) {
        return 2;
    }
    3
}

/// `new String(bytes, StandardCharsets.UTF_8)`: malformed input is replaced by U+FFFD the way the
/// Java runtime does it (one replacement per malformed sequence, surrogate encodings included).
pub fn java_utf8_decode(bytes: &[u8]) -> String {
    let src: Vec<i8> = bytes.iter().map(|&b| b as i8).collect();
    let sl = src.len();
    let mut out: Vec<u16> = Vec::with_capacity(sl);
    let mut sp = 0usize;
    while sp < sl {
        let b1 = i32::from(src[sp]);
        sp += 1;
        if b1 >= 0 {
            out.push(b1 as u16);
        } else if (b1 >> 5) == -2 && (b1 & 0x1e) != 0 {
            if sp < sl {
                let b2 = i32::from(src[sp]);
                sp += 1;
                if not_continuation(b2) {
                    out.push(REPL);
                    sp -= 1;
                } else {
                    out.push(
                        (((b1 << 6) ^ b2) ^ (((0xc0u8 as i8 as i32) << 6) ^ (0x80u8 as i8 as i32)))
                            as u16,
                    );
                }
                continue;
            }
            out.push(REPL);
            break;
        } else if (b1 >> 4) == -2 {
            if sp + 1 < sl {
                let b2 = i32::from(src[sp]);
                let b3 = i32::from(src[sp + 1]);
                sp += 2;
                if is_malformed3(b1, b2, b3) {
                    out.push(REPL);
                    sp -= 3;
                    sp += malformed3(&src, sp);
                } else {
                    let c = ((b1 << 12)
                        ^ (b2 << 6)
                        ^ (b3
                            ^ (((0xe0u8 as i8 as i32) << 12)
                                ^ ((0x80u8 as i8 as i32) << 6)
                                ^ (0x80u8 as i8 as i32)))) as u16;
                    if (0xD800..=0xDFFF).contains(&c) {
                        out.push(REPL);
                    } else {
                        out.push(c);
                    }
                }
                continue;
            }
            if sp < sl && is_malformed3_2(b1, i32::from(src[sp])) {
                out.push(REPL);
                continue;
            }
            out.push(REPL);
            break;
        } else if (b1 >> 3) == -2 {
            if sp + 2 < sl {
                let b2 = i32::from(src[sp]);
                let b3 = i32::from(src[sp + 1]);
                let b4 = i32::from(src[sp + 2]);
                sp += 3;
                let uc = (b1 << 18)
                    ^ (b2 << 12)
                    ^ (b3 << 6)
                    ^ (b4
                        ^ (((0xf0u8 as i8 as i32) << 18)
                            ^ ((0x80u8 as i8 as i32) << 12)
                            ^ ((0x80u8 as i8 as i32) << 6)
                            ^ (0x80u8 as i8 as i32)));
                if is_malformed4(b2, b3, b4) || !(0x10000..=0x10FFFF).contains(&uc) {
                    out.push(REPL);
                    sp -= 4;
                    sp += malformed4(&src, sp);
                } else {
                    let u = uc as u32 - 0x10000;
                    out.push((0xD800 + (u >> 10)) as u16);
                    out.push((0xDC00 + (u & 0x3FF)) as u16);
                }
                continue;
            }
            let b1u = b1 & 0xff;
            if b1u > 0xf4 || (sp < sl && is_malformed4_2(b1u, i32::from(src[sp]) & 0xff)) {
                out.push(REPL);
                continue;
            }
            sp += 1;
            out.push(REPL);
            if sp < sl && not_continuation(i32::from(src[sp])) {
                continue;
            }
            break;
        } else {
            out.push(REPL);
        }
    }
    String::from_utf16_lossy(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_text() {
        assert_eq!(
            java_utf8_decode("a\u{e9}\u{20ac}\u{1f600}".as_bytes()),
            "a\u{e9}\u{20ac}\u{1f600}"
        );
    }

    #[test]
    fn surrogate_encoding_is_one_replacement_per_unit() {
        // CESU-8 encoding of U+1F600 (two 3 byte surrogates)
        let bytes = [0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80];
        assert_eq!(java_utf8_decode(&bytes), "\u{fffd}\u{fffd}");
    }

    #[test]
    fn stray_bytes() {
        assert_eq!(java_utf8_decode(&[0x61, 0x80, 0x62]), "a\u{fffd}b");
        assert_eq!(java_utf8_decode(&[0xC0, 0xAF]), "\u{fffd}\u{fffd}");
        assert_eq!(java_utf8_decode(&[0xE2, 0x82]), "\u{fffd}");
    }
}
