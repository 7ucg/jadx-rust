//! Java identifier validation (`NameMapper`). Unicode categories are approximated.

const RESERVED: &[&str] = &[
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

pub fn is_reserved(s: &str) -> bool {
    RESERVED.contains(&s)
}

/// Combining marks (Unicode Mn/Mc) that may continue but not start an identifier.
fn is_mark(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x05BF | 0x05C1..=0x05C2 | 0x05C4..=0x05C5 | 0x05C7
        | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7..=0x06E8
        | 0x06EA..=0x06ED | 0x0711 | 0x0730..=0x074A | 0x0900..=0x0903 | 0x093A..=0x094F | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F)
}

fn is_ident_start(c: char) -> bool {
    !is_mark(c) && (c.is_alphabetic() || c == '_' || c == '$' || is_currency(c))
}

fn is_currency(c: char) -> bool {
    matches!(c as u32, 0x00A2..=0x00A5 | 0x058F | 0x060B | 0x09F2..=0x09F3 | 0x0E3F | 0x20A0..=0x20C0 | 0xFDFC | 0xFE69 | 0xFF04 | 0xFFE0..=0xFFE1 | 0xFFE5..=0xFFE6)
}

fn is_ident_part(c: char) -> bool {
    is_ident_start(c) || is_mark(c) || c.is_numeric() && c.is_alphanumeric() || c.is_ascii_digit()
}

fn valid_ident_chars(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if is_ident_start(c) => it.all(is_ident_part),
        _ => false,
    }
}

pub fn is_valid_identifier(s: &str) -> bool {
    !s.is_empty() && !is_reserved(s) && valid_ident_chars(s)
}

pub fn is_valid_full_identifier(s: &str) -> bool {
    !s.is_empty() && !is_reserved(s) && s.split('.').all(valid_ident_chars)
}
