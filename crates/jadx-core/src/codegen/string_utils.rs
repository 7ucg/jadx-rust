//! Literal formatting (`StringUtils`): strings, chars, numbers and floating point values as Java
//! source text.

/// `IntegerFormat`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntegerFormat {
    #[default]
    Auto,
    Decimal,
    Hexadecimal,
}

impl IntegerFormat {
    pub fn is_hexadecimal(self) -> bool {
        self == IntegerFormat::Hexadecimal
    }
}

/// Formatting options (`StringUtils` instance state).
#[derive(Debug, Clone, Copy, Default)]
pub struct StringUtils {
    pub escape_unicode: bool,
    pub integer_format: IntegerFormat,
}

/// `NameMapper.isPrintableCodePoint`: no control, format, private use, surrogate or unassigned
/// code points and no white space other than the plain space.
pub fn is_printable_code_point(cp: u32) -> bool {
    super::unicode_tables::NON_PRINTABLE
        .binary_search_by(|&(first, last)| {
            if cp < first {
                std::cmp::Ordering::Greater
            } else if cp > last {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_err()
}

/// `NameMapper.isPrintableChar`
pub fn is_printable_char(c: u16) -> bool {
    (32..=126).contains(&c)
}

/// Code points of a UTF-16 string; unpaired surrogates are reported as they are
/// (`StringUtils.visitCodePoints`).
pub fn code_points(units: &[u16]) -> Vec<u32> {
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        if (0xd800..0xdc00).contains(&u) && i + 1 < units.len() {
            let l = units[i + 1];
            if (0xdc00..0xe000).contains(&l) {
                out.push(0x10000 + ((u32::from(u) - 0xd800) << 10) + (u32::from(l) - 0xdc00));
                i += 2;
                continue;
            }
        }
        out.push(u32::from(u));
        i += 1;
    }
    out
}

fn special_for_code_point(c: u32) -> Option<&'static str> {
    match char::from_u32(c)? {
        '\n' => Some("\\n"),
        '\r' => Some("\\r"),
        '\t' => Some("\\t"),
        '\u{8}' => Some("\\b"),
        '\u{c}' => Some("\\f"),
        '\'' => Some("'"),
        '"' => Some("\\\""),
        '\\' => Some("\\\\"),
        _ => None,
    }
}

impl StringUtils {
    /// `unescapeString`: Java string literal for the UTF-16 contents.
    pub fn unescape_string(&self, units: &[u16]) -> String {
        if units.is_empty() {
            return "\"\"".to_string();
        }
        let mut res = String::with_capacity(units.len() + 2);
        res.push('"');
        for cp in code_points(units) {
            self.process_code_point(cp, &mut res);
        }
        res.push('"');
        res
    }

    /// `unescapeString` for a Rust string.
    pub fn unescape_str(&self, s: &str) -> String {
        let units: Vec<u16> = s.encode_utf16().collect();
        self.unescape_string(&units)
    }

    fn process_code_point(&self, cp: u32, res: &mut String) {
        if let Some(s) = special_for_code_point(cp) {
            res.push_str(s);
            return;
        }
        if self.is_escape_needed(cp) {
            res.push_str(&format!("\\u{cp:04x}"));
        } else if let Some(c) = char::from_u32(cp) {
            res.push(c);
        }
    }

    fn is_escape_needed(&self, cp: u32) -> bool {
        if cp < 32 {
            return true;
        }
        if cp < 127 {
            return false;
        }
        if self.escape_unicode {
            return true;
        }
        !is_printable_code_point(cp)
    }

    /// `unescapeChar`: single char literal, the best way possible.
    pub fn unescape_char(&self, c: u16, explicit_cast: bool) -> String {
        if c == u16::from(b'\'') {
            return "'\\''".to_string();
        }
        if let Some(s) = special_for_code_point(u32::from(c)) {
            return format!("'{s}'");
        }
        if c >= 127 && self.escape_unicode {
            return format!("'\\u{c:04x}'");
        }
        if is_printable_char(c) {
            return format!("'{}'", c as u8 as char);
        }
        let int_str = c.to_string();
        if explicit_cast {
            format!("(char) {int_str}")
        } else {
            int_str
        }
    }

    fn format_number(&self, number: i64, bytes_len: usize, mut cast: bool) -> String {
        let num_str = if self.integer_format.is_hexadecimal() {
            let hex = format!("{:x}", number);
            if number < 0 {
                // cut leading 'f' for negative numbers to match number type length
                let len = hex.len();
                cast = true;
                format!("0x{}", &hex[len - bytes_len * 2..])
            } else {
                format!("0x{hex}")
            }
        } else {
            number.to_string()
        };
        if bytes_len == 8 && (number == i64::MIN || number.unsigned_abs() >= i32::MAX as u64) {
            // force cast for long values bigger than min/max int
            cast = true;
        }
        if cast {
            if bytes_len == 8 {
                return format!("{num_str}L");
            }
            return format!("{}{num_str}", cast_str(bytes_len));
        }
        num_str
    }

    pub fn format_byte(&self, l: i64, cast: bool) -> String {
        self.format_number(l, 1, cast)
    }

    pub fn format_short(&self, l: i64, cast: bool) -> String {
        if self.integer_format == IntegerFormat::Auto {
            match l as i16 {
                i16::MAX => return "Short.MAX_VALUE".to_string(),
                i16::MIN => return "Short.MIN_VALUE".to_string(),
                _ => {}
            }
        }
        self.format_number(l, 2, cast)
    }

    pub fn format_integer(&self, l: i64, cast: bool) -> String {
        if self.integer_format == IntegerFormat::Auto {
            match l as i32 {
                i32::MAX => return "Integer.MAX_VALUE".to_string(),
                i32::MIN => return "Integer.MIN_VALUE".to_string(),
                _ => {}
            }
        }
        self.format_number(l, 4, cast)
    }

    pub fn format_long(&self, l: i64, cast: bool) -> String {
        if self.integer_format == IntegerFormat::Auto {
            if l == i64::MAX {
                return "Long.MAX_VALUE".to_string();
            }
            if l == i64::MIN {
                return "Long.MIN_VALUE".to_string();
            }
        }
        self.format_number(l, 8, cast)
    }
}

fn cast_str(bytes_len: usize) -> &'static str {
    match bytes_len {
        1 => "(byte) ",
        2 => "(short) ",
        4 => "(int) ",
        _ => "(long) ",
    }
}

/// `Double.MIN_NORMAL`
const DOUBLE_MIN_NORMAL: f64 = f64::MIN_POSITIVE;
/// `Float.MIN_NORMAL`
const FLOAT_MIN_NORMAL: f32 = f32::MIN_POSITIVE;

/// `StringUtils.formatDouble`
pub fn format_double(d: f64) -> String {
    if d.is_nan() {
        return "Double.NaN".to_string();
    }
    if d == f64::NEG_INFINITY {
        return "Double.NEGATIVE_INFINITY".to_string();
    }
    if d == f64::INFINITY {
        return "Double.POSITIVE_INFINITY".to_string();
    }
    if d == 4.9E-324 {
        return "Double.MIN_VALUE".to_string();
    }
    if d == f64::MAX {
        return "Double.MAX_VALUE".to_string();
    }
    if d == DOUBLE_MIN_NORMAL {
        return "Double.MIN_NORMAL".to_string();
    }
    format!("{}d", java_double_to_string(d))
}

/// `StringUtils.formatFloat`
pub fn format_float(f: f32) -> String {
    if f.is_nan() {
        return "Float.NaN".to_string();
    }
    if f == f32::NEG_INFINITY {
        return "Float.NEGATIVE_INFINITY".to_string();
    }
    if f == f32::INFINITY {
        return "Float.POSITIVE_INFINITY".to_string();
    }
    if f == 1.4E-45 {
        return "Float.MIN_VALUE".to_string();
    }
    if f == f32::MAX {
        return "Float.MAX_VALUE".to_string();
    }
    if f == FLOAT_MIN_NORMAL {
        return "Float.MIN_NORMAL".to_string();
    }
    format!("{}f", java_float_to_string(f))
}

/// `Double.toString`
pub fn java_double_to_string(d: f64) -> String {
    if d.is_nan() {
        return "NaN".to_string();
    }
    if d.is_infinite() {
        return if d > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let sign = if d.is_sign_negative() { "-" } else { "" };
    if d == 0.0 {
        return format!("{sign}0.0");
    }
    let a = d.abs();
    let mut sci = format!("{a:e}");
    if !sci.split('e').next().unwrap_or("").contains('.') {
        // a single digit: the reference prints the closest decimal with two digits
        sci = format!("{a:.1e}");
    }
    let body = java_layout(&sci, (1e-3..1e7).contains(&a));
    format!("{sign}{body}")
}

/// `Float.toString`
pub fn java_float_to_string(f: f32) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let sign = if f.is_sign_negative() { "-" } else { "" };
    if f == 0.0 {
        return format!("{sign}0.0");
    }
    let a = f.abs();
    let mut sci = format!("{a:e}");
    if !sci.split('e').next().unwrap_or("").contains('.') {
        sci = format!("{a:.1e}");
    }
    let body = java_layout(&sci, (1e-3..1e7).contains(&a));
    format!("{sign}{body}")
}

/// Lay out the digits of Rust's scientific notation (`1.234e-5`) the way Java does: plain decimal
/// notation for `1e-3 <= x < 1e7`, otherwise `d.dddE<exp>`.
fn java_layout(sci: &str, plain: bool) -> String {
    let (mant, exp) = sci.split_once('e').unwrap_or((sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    // the two digit form of a one digit value has no significant second digit
    let digits = match digits.trim_end_matches('0') {
        "" => "0".to_string(),
        d => d.to_string(),
    };
    if !plain {
        let (first, rest) = digits.split_at(1);
        let rest = if rest.is_empty() { "0" } else { rest };
        return format!("{first}.{rest}E{exp}");
    }
    if exp >= 0 {
        let int_len = exp as usize + 1;
        if digits.len() <= int_len {
            let zeros = "0".repeat(int_len - digits.len());
            format!("{digits}{zeros}.0")
        } else {
            format!("{}.{}", &digits[..int_len], &digits[int_len..])
        }
    } else {
        format!("0.{}{}", "0".repeat((-exp - 1) as usize), digits)
    }
}

/// `StringUtils.escape`: make a valid identifier part out of a type name.
pub fn escape_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '.' | '/' | ';' | '$' | ' ' | ',' | '<' => out.push('_'),
            '[' => out.push('A'),
            ']' | '>' | '?' | '*' => {}
            _ => out.push(c),
        }
    }
    out
}

/// `StringUtils.capitalizeFirstChar`
pub fn capitalize_first_char(s: &str) -> String {
    let mut it = s.chars();
    match it.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().chain(it).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_like_java() {
        assert_eq!(java_double_to_string(1.0), "1.0");
        assert_eq!(java_double_to_string(0.5), "0.5");
        assert_eq!(java_double_to_string(100.0), "100.0");
        assert_eq!(java_double_to_string(1234567.0), "1234567.0");
        assert_eq!(java_double_to_string(1.0E7), "1.0E7");
        assert_eq!(java_double_to_string(12345678.9), "1.23456789E7");
        assert_eq!(java_double_to_string(0.001), "0.001");
        assert_eq!(java_double_to_string(0.0001), "1.0E-4");
        assert_eq!(java_double_to_string(-2.5), "-2.5");
        assert_eq!(java_double_to_string(1.0E23), "1.0E23");
        assert_eq!(java_double_to_string(-0.0), "-0.0");
        assert_eq!(java_double_to_string(0.1), "0.1");
        assert_eq!(java_double_to_string(2.75), "2.75");
        assert_eq!(java_float_to_string(0.1), "0.1");
        assert_eq!(java_float_to_string(3.4e38), "3.4E38");
        assert_eq!(java_float_to_string(16777216.0), "1.6777216E7");
        assert_eq!(format_double(f64::NAN), "Double.NaN");
        assert_eq!(format_float(1.5), "1.5f");
        assert_eq!(format_double(2.0), "2.0d");
    }

    #[test]
    fn strings_and_chars() {
        let su = StringUtils::default();
        assert_eq!(su.unescape_str("a\"b\\c\n\t'"), "\"a\\\"b\\\\c\\n\\t'\"");
        assert_eq!(su.unescape_str(""), "\"\"");
        assert_eq!(su.unescape_str("\u{1}é"), "\"\\u0001é\"");
        assert_eq!(su.unescape_string(&[0xd800]), "\"\\ud800\"");
        assert_eq!(su.unescape_char(u16::from(b'a'), false), "'a'");
        assert_eq!(su.unescape_char(u16::from(b'\''), false), "'\\''");
        assert_eq!(su.unescape_char(0, true), "(char) 0");
        assert_eq!(su.unescape_char(7, false), "7");
        assert_eq!(su.unescape_char(u16::from(b'\n'), false), "'\\n'");
    }

    #[test]
    fn integers() {
        let su = StringUtils::default();
        assert_eq!(su.format_integer(5, false), "5");
        assert_eq!(
            su.format_integer(i64::from(i32::MAX), false),
            "Integer.MAX_VALUE"
        );
        assert_eq!(su.format_integer(-3, true), "(int) -3");
        assert_eq!(su.format_long(5, false), "5");
        assert_eq!(su.format_long(5_000_000_000, false), "5000000000L");
        assert_eq!(su.format_byte(-1, true), "(byte) -1");
        let hex = StringUtils {
            integer_format: IntegerFormat::Hexadecimal,
            ..StringUtils::default()
        };
        assert_eq!(hex.format_integer(255, false), "0xff");
        assert_eq!(hex.format_integer(-1, false), "(int) 0xffffffff");
    }

    #[test]
    fn names() {
        assert_eq!(
            escape_name("java.util.List<String>"),
            "java_util_List_String"
        );
        assert_eq!(escape_name("a[]"), "aA");
        assert_eq!(capitalize_first_char("abc"), "Abc");
    }
}
