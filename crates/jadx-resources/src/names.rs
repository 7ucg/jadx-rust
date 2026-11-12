//! Resource name handling: validity rules (`ResNameUtils`, `NameMapper`) and the choice between
//! the name in the resource table and the name of the matching code field (`BetterName`).

use crate::xml_chars::{NAME_CHAR, NAME_START};

const RESERVED_NAMES: [&str; 54] = [
    "_",
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "false",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "true",
    "try",
    "void",
    "volatile",
    "while",
];

/// Where the final resource name comes from when the app code also names the resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResourceNameSource {
    /// The better looking of both names.
    #[default]
    Auto,
    /// The name in the resource table.
    Resources,
    /// The name of the code field.
    Code,
}

/// `NameMapper.isReserved`
pub fn is_reserved(s: &str) -> bool {
    RESERVED_NAMES.contains(&s)
}

fn in_ranges(table: &[(u16, u16)], c: u16) -> bool {
    table
        .binary_search_by(|&(lo, hi)| {
            if c < lo {
                std::cmp::Ordering::Greater
            } else if c > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `XMLChar.isValidName` (XML 1.0 name production on UTF-16 units).
pub fn is_valid_xml_name(name: &str) -> bool {
    let mut it = name.encode_utf16();
    match it.next() {
        Some(first) if in_ranges(&NAME_START, first) => {}
        _ => return false,
    }
    it.all(|u| in_ranges(&NAME_CHAR, u))
}

/// Unicode letter (general category L*) below U+0370.
fn is_letter_low(cp: u32) -> bool {
    matches!(cp,
        0x41..=0x5A | 0x61..=0x7A | 0xAA | 0xB5 | 0xBA | 0xC0..=0xD6 | 0xD8..=0xF6
        | 0xF8..=0x2C1 | 0x2C6..=0x2D1 | 0x2E0..=0x2E4 | 0x2EC | 0x2EE)
}

fn is_printable_ascii(cp: u32) -> bool {
    (32..=126).contains(&cp)
}

/// First character of a resource name (aapt2 rules and valid in an R class field name).
/// Only characters below U+0370 can be valid.
fn is_valid_resource_name_start(cp: u32, allow_non_printable: bool) -> bool {
    (allow_non_printable || is_printable_ascii(cp)) && (is_letter_low(cp) || cp == '_' as u32)
}

fn is_valid_resource_name_part(cp: u32, allow_non_printable: bool) -> bool {
    (allow_non_printable || is_printable_ascii(cp))
        && (is_letter_low(cp)
            || (0x30..=0x39).contains(&cp)
            || cp == '_' as u32
            || (0x300..=0x36F).contains(&cp)
            || cp == '.' as u32)
}

/// `ResNameUtils.sanitizeAsResourceName`: keeps valid names, otherwise replaces invalid
/// characters by `_` and appends `postfix`.
pub fn sanitize_as_resource_name(name: &str, postfix: &str, allow_non_printable: bool) -> String {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return postfix.to_string();
    };
    let mut sb = String::with_capacity(name.len() + 1);
    let mut changed = false;
    let cp = first as u32;
    if is_valid_resource_name_start(cp, allow_non_printable) {
        sb.push(first);
    } else {
        sb.push('_');
        changed = true;
        if is_valid_resource_name_part(cp, allow_non_printable) {
            sb.push(first);
        }
    }
    for c in chars {
        if is_valid_resource_name_part(c as u32, allow_non_printable) {
            sb.push(c);
        } else {
            sb.push('_');
            changed = true;
        }
    }
    if is_reserved(&sb) {
        changed = true;
    }
    if changed {
        sb.push_str(postfix);
    }
    sb
}

/// `ResNameUtils.convertToRFieldName`
pub fn to_r_field_name(resource_name: &str) -> String {
    resource_name.replace('.', "_")
}

/// `NameMapper.isValidFullIdentifier` for the package name check (identifier parts separated by
/// dots).
pub fn is_valid_full_identifier(s: &str) -> bool {
    if s.is_empty() || is_reserved(s) {
        return false;
    }
    s.split('.').all(is_java_identifier_no_reserved_check)
}

fn is_java_identifier_no_reserved_check(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if is_java_identifier_start(c) => {}
        _ => return false,
    }
    it.all(is_java_identifier_part)
}

fn is_java_identifier_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$' || ('\u{a2}'..='\u{a5}').contains(&c)
}

fn is_java_identifier_part(c: char) -> bool {
    is_java_identifier_start(c)
        || c.is_numeric()
        || (c as u32) < 9
        || (0xE..=0x1B).contains(&(c as u32))
}

/// `BetterName.getBetterResourceName`: prefers the name with the lower share of digits, then the
/// longer one; `first` wins ties.
pub fn better_resource_name<'a>(first: &'a str, second: &'a str) -> &'a str {
    if first == second {
        return first;
    }
    if first.is_empty() || second.is_empty() {
        return if !first.is_empty() { first } else { second };
    }
    let (fd, fl) = analyze(first);
    let (sd, sl) = analyze(second);
    if fd != 0 || sd != 0 {
        let first_ratio = fd as f32 / fl as f32;
        let second_ratio = sd as f32 / sl as f32;
        if f64::from((second_ratio - first_ratio).abs()) >= 0.001 {
            return if first_ratio <= second_ratio {
                first
            } else {
                second
            };
        }
    }
    if fl >= sl {
        first
    } else {
        second
    }
}

fn analyze(s: &str) -> (u32, u32) {
    let mut digits = 0;
    let mut len = 0;
    for c in s.chars() {
        if c.is_ascii_digit() {
            digits += 1;
        }
        len += 1;
    }
    (digits, len)
}

/// `ResTableBinaryParser.getBetterName`
pub fn better_name<'a>(
    source: ResourceNameSource,
    res_name: &'a str,
    code_name: &'a str,
) -> &'a str {
    match source {
        ResourceNameSource::Auto => better_resource_name(res_name, code_name),
        ResourceNameSource::Resources => res_name,
        ResourceNameSource::Code => code_name,
    }
}

/// `JadxZipSecurity.isValidEntryName`: no path traversal, no absolute path.
pub fn is_valid_entry_name(name: &str) -> bool {
    if name.contains("../") || name.contains("..\\") {
        return false;
    }
    if name.contains('\0') || name.starts_with('/') {
        return false;
    }
    let mut depth: i64 = 0;
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => depth += 1,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_as_resource_name("abc_1.x", "_P", false), "abc_1.x");
        assert_eq!(sanitize_as_resource_name("a-b", "_P", false), "a_b_P");
        assert_eq!(sanitize_as_resource_name("1ab", "_P", false), "_1ab_P");
        assert_eq!(sanitize_as_resource_name("switch", "_P", false), "switch_P");
        assert_eq!(sanitize_as_resource_name("", "_P", false), "_P");
        assert_eq!(sanitize_as_resource_name("a\u{e9}", "_P", false), "a__P");
        assert_eq!(sanitize_as_resource_name("a\u{e9}", "_P", true), "a\u{e9}");
    }

    #[test]
    fn xml_names() {
        assert!(is_valid_xml_name("manifest"));
        assert!(is_valid_xml_name("a-b.c:d"));
        assert!(!is_valid_xml_name(""));
        assert!(!is_valid_xml_name("1a"));
        assert!(!is_valid_xml_name("a b"));
    }

    #[test]
    fn better_names() {
        assert_eq!(better_resource_name("abc", "ab12"), "abc");
        assert_eq!(better_resource_name("a1", "abcd"), "abcd");
        assert_eq!(better_resource_name("ab", "cd"), "ab");
    }

    #[test]
    fn entry_names() {
        assert!(is_valid_entry_name("res/layout/a.xml"));
        assert!(!is_valid_entry_name("../a"));
        assert!(!is_valid_entry_name("/a"));
        assert!(!is_valid_entry_name("a/../../b"));
        assert!(!is_valid_entry_name("a/../b"));
    }

    #[test]
    fn packages() {
        assert!(is_valid_full_identifier("com.example.app"));
        assert!(!is_valid_full_identifier("com..x"));
        assert!(!is_valid_full_identifier(""));
    }
}
