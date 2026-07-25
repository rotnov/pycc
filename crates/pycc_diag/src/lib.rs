#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub span: Option<Span>,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self { code, severity: Severity::Error, message: message.into(), span: Some(span) }
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, severity: Severity::Warning, message: message.into(), span: None }
    }
}

/// 1-indexed line and column, computed from a byte offset into `source` --
/// CLI_SPEC.md's human format shows `src/main.py:5:15` (1-indexed line:col),
/// and the JSON format's `spans[{line,col,...}]` needs the same. Hand-rolled
/// rather than pulling in `ruff_source_file`'s `LineIndex` (a separate crate
/// this workspace doesn't otherwise depend on) for one small function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub column: u32,
}

pub fn byte_offset_to_line_col(source: &str, offset: u32) -> LineCol {
    let offset = offset as usize;
    let mut line = 1u32;
    let mut last_newline_end = 0usize;
    for (i, b) in source.bytes().enumerate() {
        if i >= offset {
            break;
        }
        if b == b'\n' {
            line += 1;
            last_newline_end = i + 1;
        }
    }
    let column = (offset.saturating_sub(last_newline_end)) as u32 + 1;
    LineCol { line, column }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_carries_code_severity_and_span() {
        let d = Diagnostic::error("T0001", "argument missing annotation", Span::new(10, 14));
        assert_eq!(d.code, "T0001");
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.span, Some(Span::new(10, 14)));
    }

    #[test]
    fn diagnostic_without_span_is_allowed() {
        let d = Diagnostic::warning("W1001", "unreachable code");
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.span, None);
    }

    #[test]
    fn byte_offset_to_line_col_finds_first_line_first_column() {
        assert_eq!(byte_offset_to_line_col("print(42)\n", 0), LineCol { line: 1, column: 1 });
    }

    #[test]
    fn byte_offset_to_line_col_finds_a_later_line() {
        let source = "def f():\n    print(42)\n";
        // offset 13 is the 'p' in "print", on line 2, column 5 (1-indexed, after 4 spaces)
        assert_eq!(byte_offset_to_line_col(source, 13), LineCol { line: 2, column: 5 });
    }

    #[test]
    fn byte_offset_to_line_col_at_a_newline_byte_stays_on_the_line_it_ends() {
        let source = "ab\ncd";
        // offset 2 is the '\n' itself -- still counted as the end of line 1, column 3
        assert_eq!(byte_offset_to_line_col(source, 2), LineCol { line: 1, column: 3 });
    }
}
