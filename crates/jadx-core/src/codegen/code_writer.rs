//! Code writer (`ICodeWriter` without metadata): indentation, line bookkeeping and the map of
//! decompiled lines to source lines.

use std::collections::BTreeMap;

pub const DEFAULT_INDENT_STR: &str = "    ";
pub const DEFAULT_NEW_LINE_STR: &str = "\n";

#[derive(Debug, Clone)]
pub struct CodeWriter {
    buf: String,
    /// Length of `buf` in UTF-16 code units (`StringBuilder.length`).
    len: usize,
    indent: usize,
    indent_str: String,
    single_indent: String,
    new_line_str: String,
    insert_line_numbers: bool,
    /// Current line, counting from 1.
    line: usize,
    /// Number of units written to the current line.
    offset: usize,
    line_map: BTreeMap<usize, u32>,
}

impl Default for CodeWriter {
    fn default() -> Self {
        CodeWriter::new()
    }
}

/// Finished code with the decompiled line to source line map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeInfo {
    pub code: String,
    pub line_map: BTreeMap<usize, u32>,
}

impl CodeWriter {
    pub fn new() -> Self {
        Self::with_options(DEFAULT_INDENT_STR, DEFAULT_NEW_LINE_STR, false)
    }

    pub fn with_options(indent_str: &str, new_line_str: &str, insert_line_numbers: bool) -> Self {
        let mut w = CodeWriter {
            buf: String::new(),
            len: 0,
            indent: 0,
            indent_str: String::new(),
            single_indent: indent_str.to_string(),
            new_line_str: new_line_str.to_string(),
            insert_line_numbers,
            line: 1,
            offset: 0,
            line_map: BTreeMap::new(),
        };
        if insert_line_numbers {
            w.inc_indent_n(3);
            let s = w.indent_str.clone();
            w.add(&s);
        }
        w
    }

    pub fn start_line(&mut self) -> &mut Self {
        self.add_line();
        self.add_line_indent();
        self
    }

    pub fn start_line_char(&mut self, c: char) -> &mut Self {
        self.start_line();
        self.add_char(c);
        self
    }

    pub fn start_line_str(&mut self, s: &str) -> &mut Self {
        self.start_line();
        self.add(s);
        self
    }

    /// `startLineWithNum`: starts a line and maps it to the source line (`0` = unknown).
    pub fn start_line_with_num(&mut self, source_line: u32) -> &mut Self {
        if source_line == 0 {
            self.start_line();
            return self;
        }
        if self.insert_line_numbers {
            self.new_line();
            self.attach_source_line(source_line);
            let start = self.len;
            self.add("/* ").add(&source_line.to_string()).add(" */ ");
            let len = self.len - start;
            if self.indent_str.len() > len {
                let rest = self.indent_str[len..].to_string();
                self.add(&rest);
            }
        } else {
            self.start_line();
            self.attach_source_line(source_line);
        }
        self
    }

    /// `addMultiLine`: new lines inside the text continue with the current indentation.
    pub fn add_multi_line(&mut self, s: &str) -> &mut Self {
        if s.contains(&self.new_line_str) {
            let replaced = s.replace(
                &self.new_line_str,
                &format!("{}{}", self.new_line_str, self.indent_str),
            );
            self.buf.push_str(&replaced);
            self.len += utf16_len(&replaced);
            self.line += s.matches(&self.new_line_str).count();
            self.offset = 0;
        } else {
            self.add(s);
        }
        self
    }

    pub fn add(&mut self, s: &str) -> &mut Self {
        self.buf.push_str(s);
        let n = utf16_len(s);
        self.len += n;
        self.offset += n;
        self
    }

    pub fn add_char(&mut self, c: char) -> &mut Self {
        self.buf.push(c);
        let n = c.len_utf16();
        self.len += n;
        self.offset += n;
        self
    }

    /// `add(ICodeWriter)`: append another writer's code and merge its line map.
    pub fn add_writer(&mut self, other: &CodeWriter) -> &mut Self {
        self.line -= 1;
        for (l, src) in &other.line_map {
            self.line_map.insert(self.line + l, *src);
        }
        self.line += other.line;
        self.offset = other.offset;
        self.buf.push_str(&other.buf);
        self.len += other.len;
        self
    }

    pub fn new_line(&mut self) -> &mut Self {
        self.add_line();
        self
    }

    pub fn add_indent(&mut self) -> &mut Self {
        let s = self.single_indent.clone();
        self.add(&s);
        self
    }

    fn add_line(&mut self) {
        self.buf.push_str(&self.new_line_str);
        self.len += utf16_len(&self.new_line_str);
        self.line += 1;
        self.offset = 0;
    }

    fn add_line_indent(&mut self) {
        self.buf.push_str(&self.indent_str);
        let n = utf16_len(&self.indent_str);
        self.len += n;
        self.offset += n;
    }

    fn update_indent(&mut self) {
        self.indent_str = self.single_indent.repeat(self.indent);
    }

    pub fn inc_indent(&mut self) {
        self.inc_indent_n(1);
    }

    fn inc_indent_n(&mut self, c: usize) {
        self.indent += c;
        self.update_indent();
    }

    pub fn dec_indent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
        self.update_indent();
    }

    pub fn indent(&self) -> usize {
        self.indent
    }

    pub fn set_indent(&mut self, indent: usize) {
        self.indent = indent;
        self.update_indent();
    }

    /// Current line (counting from 1).
    pub fn line(&self) -> usize {
        self.line
    }

    /// Position of the first unit of the current line.
    pub fn line_start_pos(&self) -> usize {
        self.len - self.offset
    }

    /// `attachSourceLine` for the current line.
    pub fn attach_source_line(&mut self, source_line: u32) {
        if source_line == 0 {
            return;
        }
        self.line_map.insert(self.line, source_line);
    }

    pub fn line_map(&self) -> &BTreeMap<usize, u32> {
        &self.line_map
    }

    /// Length in UTF-16 code units (`getLength`).
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_str(&self) -> &str {
        &self.buf
    }

    pub fn finish(self) -> CodeInfo {
        CodeInfo {
            code: self.buf,
            line_map: self.line_map,
        }
    }
}

fn utf16_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.encode_utf16().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_indent() {
        let mut w = CodeWriter::new();
        w.add("class A {");
        w.inc_indent();
        w.start_line_str("int a;");
        w.start_line_with_num(7).add("a = 1;");
        w.dec_indent();
        w.start_line_char('}');
        assert_eq!(w.as_str(), "class A {\n    int a;\n    a = 1;\n}");
        assert_eq!(w.line(), 4);
        assert_eq!(w.line_map().get(&3), Some(&7));
        assert_eq!(w.line_start_pos(), w.len() - 1);
    }

    #[test]
    fn multi_line_and_merge() {
        let mut w = CodeWriter::new();
        w.inc_indent();
        w.start_line();
        w.add_multi_line("a\nb");
        assert_eq!(w.as_str(), "\n    a\n    b");
        assert_eq!(w.line(), 3);
        let mut inner = CodeWriter::new();
        inner.add("x").start_line_with_num(3).add("y");
        w.new_line();
        w.add_writer(&inner);
        assert!(w.as_str().ends_with("x\ny"));
        assert_eq!(w.line(), 5);
        assert_eq!(w.line_map().get(&5), Some(&3));
    }

    #[test]
    fn utf16_length() {
        let mut w = CodeWriter::new();
        w.add("é\u{1F600}");
        assert_eq!(w.len(), 3);
    }
}
