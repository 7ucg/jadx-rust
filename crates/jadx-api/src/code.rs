//! Generated code containers: `ICodeInfo` and the line oriented `CodeWriter`.

/// Result of the code generation of a class.
pub trait ICodeInfo: Send + Sync {
    fn code_str(&self) -> &str;

    /// True if code metadata (annotations, line mapping) is available.
    fn has_metadata(&self) -> bool {
        false
    }
}

/// Code text without metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimpleCodeInfo {
    code: String,
}

impl SimpleCodeInfo {
    pub fn new(code: impl Into<String>) -> Self {
        SimpleCodeInfo { code: code.into() }
    }

    /// No code: nothing is saved for the class (`ICodeInfo.EMPTY`).
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.code.is_empty()
    }

    pub fn into_string(self) -> String {
        self.code
    }
}

impl ICodeInfo for SimpleCodeInfo {
    fn code_str(&self) -> &str {
        &self.code
    }
}

/// Appends code text with indentation handling.
#[derive(Debug, Clone)]
pub struct CodeWriter {
    buf: String,
    indent: usize,
    indent_str: String,
    new_line_str: String,
}

impl Default for CodeWriter {
    fn default() -> Self {
        CodeWriter::new("    ", "\n")
    }
}

impl CodeWriter {
    pub fn new(indent_str: &str, new_line_str: &str) -> Self {
        CodeWriter {
            buf: String::new(),
            indent: 0,
            indent_str: indent_str.to_string(),
            new_line_str: new_line_str.to_string(),
        }
    }

    /// Append text to the current line.
    pub fn add(&mut self, s: &str) -> &mut Self {
        self.buf.push_str(s);
        self
    }

    pub fn add_char(&mut self, c: char) -> &mut Self {
        self.buf.push(c);
        self
    }

    /// End the current line (if any text was written) and start an indented one.
    pub fn start_line(&mut self) -> &mut Self {
        if !self.buf.is_empty() {
            self.buf.push_str(&self.new_line_str);
        }
        self.add_indent()
    }

    /// `start_line` followed by `s`.
    pub fn start_line_str(&mut self, s: &str) -> &mut Self {
        self.start_line();
        self.add(s)
    }

    /// Add an empty line.
    pub fn new_line(&mut self) -> &mut Self {
        self.buf.push_str(&self.new_line_str);
        self
    }

    fn add_indent(&mut self) -> &mut Self {
        for _ in 0..self.indent {
            self.buf.push_str(&self.indent_str);
        }
        self
    }

    pub fn inc_indent(&mut self) {
        self.indent += 1;
    }

    pub fn dec_indent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
    }

    pub fn indent_level(&self) -> usize {
        self.indent
    }

    /// Length of the text written so far.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn finish(self) -> SimpleCodeInfo {
        SimpleCodeInfo::new(self.buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_indents() {
        let mut w = CodeWriter::default();
        w.start_line_str("class A {");
        w.inc_indent();
        w.start_line_str("int f;");
        w.dec_indent();
        w.start_line_str("}");
        assert_eq!(w.finish().code_str(), "class A {\n    int f;\n}");
    }
}
