//! Text helpers: XML/resource value escaping, the indenting code writer and the format string
//! substitution check.

use std::fmt::Write as _;

/// `StringUtils.escapeXML`
pub fn escape_xml(s: &str) -> String {
    let mut sb = String::with_capacity(s.len());
    for c in s.chars() {
        match escape_xml_char(c) {
            Some(r) => sb.push_str(&r),
            None => sb.push(c),
        }
    }
    sb
}

/// `StringUtils.escapeResValue`
pub fn escape_res_value(s: &str) -> String {
    let mut sb = String::with_capacity(s.len());
    for c in s.chars() {
        common_escape_and_append(&mut sb, c);
    }
    sb
}

/// `StringUtils.escapeResStrValue`
pub fn escape_res_str_value(s: &str) -> String {
    let mut sb = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => sb.push_str("\\\""),
            '\'' => sb.push_str("\\'"),
            _ => common_escape_and_append(&mut sb, c),
        }
    }
    sb
}

fn escape_xml_char(c: char) -> Option<String> {
    if (c as u32) <= 0x1F {
        return Some(format!("\\{}", c as u32));
    }
    match c {
        '&' => Some("&amp;".to_string()),
        '<' => Some("&lt;".to_string()),
        '>' => Some("&gt;".to_string()),
        '"' => Some("&quot;".to_string()),
        '\'' => Some("&apos;".to_string()),
        '\\' => Some("\\\\".to_string()),
        _ => None,
    }
}

fn escape_white_space_char(c: char) -> Option<&'static str> {
    match c {
        '\n' => Some("\\n"),
        '\r' => Some("\\r"),
        '\t' => Some("\\t"),
        '\u{8}' => Some("\\b"),
        '\u{c}' => Some("\\f"),
        _ => None,
    }
}

fn common_escape_and_append(sb: &mut String, c: char) {
    if let Some(r) = escape_white_space_char(c) {
        sb.push_str(r);
    } else if let Some(r) = escape_xml_char(c) {
        sb.push_str(&r);
    } else {
        sb.push(c);
    }
}

/// Indenting text builder (`SimpleCodeWriter` of the reference: 4 space indent, `\n` newlines).
pub struct CodeWriter {
    buf: String,
    indent: usize,
    indent_str: String,
}

const SINGLE_INDENT: &str = "    ";

impl Default for CodeWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeWriter {
    pub fn new() -> Self {
        CodeWriter {
            buf: String::new(),
            indent: 0,
            indent_str: String::new(),
        }
    }

    pub fn add(&mut self, s: &str) -> &mut Self {
        self.buf.push_str(s);
        self
    }

    pub fn add_char(&mut self, c: char) -> &mut Self {
        self.buf.push(c);
        self
    }

    /// New line followed by the current indentation.
    pub fn start_line(&mut self) -> &mut Self {
        self.buf.push('\n');
        self.buf.push_str(&self.indent_str);
        self
    }

    pub fn start_line_str(&mut self, s: &str) -> &mut Self {
        self.start_line();
        self.add(s)
    }

    pub fn start_line_char(&mut self, c: char) -> &mut Self {
        self.start_line();
        self.add_char(c)
    }

    /// One extra indentation unit at the current position.
    pub fn add_indent(&mut self) -> &mut Self {
        self.buf.push_str(SINGLE_INDENT);
        self
    }

    pub fn inc_indent(&mut self) {
        self.indent += 1;
        self.update_indent();
    }

    pub fn dec_indent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
        self.update_indent();
    }

    pub fn indent(&self) -> usize {
        self.indent
    }

    fn update_indent(&mut self) {
        self.indent_str = SINGLE_INDENT.repeat(self.indent);
    }

    /// The text, without a leading empty line.
    pub fn finish(self) -> String {
        match self.buf.strip_prefix('\n') {
            Some(rest) if !self.buf.is_empty() && self.buf.len() > 1 => rest.to_string(),
            _ => self.buf,
        }
    }
}

/// Port of the substitution scan used by apktool: `true` if the string has at least one
/// non-positional `%` substitution and more than one substitution overall.
pub fn has_multiple_non_positional_substitutions(s: &str) -> bool {
    let (non_pos, pos) = find_substitutions(s, 4);
    !non_pos.is_empty() && non_pos.len() + pos.len() > 1
}

fn find_substitutions(s: &str, non_pos_max: usize) -> (Vec<usize>, Vec<usize>) {
    let units: Vec<u16> = s.encode_utf16().collect();
    let length = units.len();
    let mut non_positional = Vec::new();
    let mut positional = Vec::new();
    let mut pos2 = 0usize;
    while let Some(rel) = units[pos2.min(length)..]
        .iter()
        .position(|&u| u == '%' as u16)
    {
        let pos = pos2 + rel;
        pos2 = pos + 1;
        if pos2 == length {
            non_positional.push(pos);
            break;
        }
        let mut c = units[pos2];
        pos2 += 1;
        if c == '%' as u16 {
            continue;
        }
        if is_digit(c) && pos2 < length {
            loop {
                c = units[pos2];
                pos2 += 1;
                if !(is_digit(c) && pos2 < length) {
                    break;
                }
            }
            if c == '$' as u16 {
                positional.push(pos);
                continue;
            }
        }
        non_positional.push(pos);
        if non_positional.len() >= non_pos_max {
            break;
        }
    }
    (non_positional, positional)
}

fn is_digit(c: u16) -> bool {
    (b'0' as u16..=b'9' as u16).contains(&c)
}

/// Hex helper matching `Integer.toHexString`.
pub fn hex32(v: i32) -> String {
    let mut s = String::new();
    let _ = write!(s, "{:x}", v as u32);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        assert_eq!(
            escape_xml("a<b>&\"'\\\n"),
            "a&lt;b&gt;&amp;&quot;&apos;\\\\\\10"
        );
        assert_eq!(escape_res_str_value("it's \"x\"\n"), "it\\'s \\\"x\\\"\\n");
        assert_eq!(escape_res_value("a'b\t"), "a&apos;b\\t");
    }

    #[test]
    fn substitutions() {
        assert!(has_multiple_non_positional_substitutions("%s and %s"));
        assert!(!has_multiple_non_positional_substitutions("%1$s and %2$s"));
        assert!(!has_multiple_non_positional_substitutions("100%%"));
        assert!(!has_multiple_non_positional_substitutions("%s"));
        assert!(has_multiple_non_positional_substitutions("%s and %1$s"));
    }

    #[test]
    fn writer_indent() {
        let mut w = CodeWriter::new();
        w.add("<a>");
        w.inc_indent();
        w.start_line_str("<b/>");
        w.dec_indent();
        w.start_line_str("</a>");
        assert_eq!(w.finish(), "<a>\n    <b/>\n</a>");
    }
}
