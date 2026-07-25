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
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            span: Some(span),
        }
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            message: message.into(),
            span: None,
        }
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

/// CLI_SPEC.md's human diagnostic format, reproduced byte-for-byte for the
/// primary error + location block: `error[CODE]: message` / ` --> file:
/// line:col` / a blank gutter line / the source line prefixed with its line
/// number / a caret-underline beneath the span. `help:` lines and a trailing
/// label after the carets are not rendered here -- no code path in this PR
/// populates one yet (see D-043).
pub fn render_human(diag: &Diagnostic, file_path: &str, source: &str) -> String {
    let mut out = String::new();
    let severity_word = match diag.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    out.push_str(&format!(
        "{severity_word}[{}]: {}\n",
        diag.code, diag.message
    ));
    let Some(span) = diag.span else {
        return out;
    };
    let start = byte_offset_to_line_col(source, span.start);
    let end = byte_offset_to_line_col(source, span.end);
    out.push_str(&format!(
        " --> {file_path}:{}:{}\n",
        start.line, start.column
    ));
    let gutter_pad = " ".repeat(start.line.to_string().len());
    out.push_str(&gutter_pad);
    out.push_str(" |\n");
    let source_line = source.lines().nth((start.line - 1) as usize).unwrap_or("");
    out.push_str(&format!("{} | {source_line}\n", start.line));
    out.push_str(&gutter_pad);
    out.push_str(" | ");
    out.push_str(&" ".repeat((start.column - 1) as usize));
    let caret_len = if end.line == start.line {
        (end.column.saturating_sub(start.column)).max(1) as usize
    } else {
        1
    };
    out.push_str(&"^".repeat(caret_len));
    out.push('\n');
    out
}

/// CLI_SPEC.md's versioned JSON diagnostic format: `format_version: 1`,
/// `spans[{file,line,col,len,label}]`. `help` is always an empty array in
/// this PR -- see `render_human`'s doc comment and D-043.
pub fn render_json(diag: &Diagnostic, file_path: &str, source: &str) -> String {
    let severity_word = match diag.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let spans = if let Some(span) = diag.span {
        let start = byte_offset_to_line_col(source, span.start);
        serde_json::json!([{
            "file": file_path,
            "line": start.line,
            "col": start.column,
            "len": span.end.saturating_sub(span.start),
            "label": serde_json::Value::Null,
        }])
    } else {
        serde_json::json!([])
    };
    let value = serde_json::json!({
        "format_version": 1,
        "code": diag.code,
        "severity": severity_word,
        "message": diag.message,
        "spans": spans,
        "help": [],
    });
    value.to_string()
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
        assert_eq!(
            byte_offset_to_line_col("print(42)\n", 0),
            LineCol { line: 1, column: 1 }
        );
    }

    #[test]
    fn byte_offset_to_line_col_finds_a_later_line() {
        let source = "def f():\n    print(42)\n";
        // offset 13 is the 'p' in "print", on line 2, column 5 (1-indexed, after 4 spaces)
        assert_eq!(
            byte_offset_to_line_col(source, 13),
            LineCol { line: 2, column: 5 }
        );
    }

    #[test]
    fn byte_offset_to_line_col_at_a_newline_byte_stays_on_the_line_it_ends() {
        let source = "ab\ncd";
        // offset 2 is the '\n' itself -- still counted as the end of line 1, column 3
        assert_eq!(
            byte_offset_to_line_col(source, 2),
            LineCol { line: 1, column: 3 }
        );
    }

    #[test]
    fn render_human_matches_cli_spec_format() {
        let source = "def fib(n):\n    print(fib(\"35\"))\n";
        // Byte offsets 26..30 slice out exactly `"35"` (with quotes) from
        // this source -- verified with a standalone script before trusting
        // this test, not counted by hand: the token starts at column 15 of
        // line 2 ("    print(fib(" is 14 chars, so the opening quote is the
        // 15th).
        assert_eq!(&source[26..30], "\"35\"");
        let diag = Diagnostic::error(
            "T0021",
            "argument 1 of `fib` expects `int`, got `str`".to_string(),
            Span::new(26, 30),
        );
        let rendered = render_human(&diag, "src/main.py", source);
        let expected = "\
error[T0021]: argument 1 of `fib` expects `int`, got `str`
 --> src/main.py:2:15
  |
2 |     print(fib(\"35\"))
  |               ^^^^
";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn render_human_with_a_multi_line_span_underlines_a_single_caret() {
        let source = "x\ny\n";
        // start = offset 0 = line 1, column 1; end = offset 2 = line 2,
        // column 1 -- lines differ, so only the start line's single caret
        // is meaningful for a one-line gutter display, rather than
        // attempting a (not-yet-supported) multi-line highlight.
        let diag = Diagnostic::error("T0001", "multi-line span".to_string(), Span::new(0, 2));
        let rendered = render_human(&diag, "src/main.py", source);
        let expected = "\
error[T0001]: multi-line span
 --> src/main.py:1:1
  |
1 | x
  | ^
";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn render_human_with_no_span_omits_the_location_block() {
        let diag = Diagnostic::warning("W1001", "unreachable code");
        let rendered = render_human(&diag, "src/main.py", "x = 1\n");
        assert_eq!(rendered, "warning[W1001]: unreachable code\n");
    }

    #[test]
    fn render_json_matches_the_versioned_schema() {
        let source = "print(1)\n";
        let diag = Diagnostic::error("T0001", "missing annotation".to_string(), Span::new(0, 5));
        let rendered = render_json(&diag, "src/main.py", source);
        let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed["format_version"], 1);
        assert_eq!(parsed["code"], "T0001");
        assert_eq!(parsed["severity"], "error");
        assert_eq!(parsed["message"], "missing annotation");
        assert_eq!(parsed["spans"][0]["file"], "src/main.py");
        assert_eq!(parsed["spans"][0]["line"], 1);
        assert_eq!(parsed["spans"][0]["col"], 1);
        assert_eq!(parsed["spans"][0]["len"], 5);
    }

    #[test]
    fn render_json_with_no_span_has_an_empty_spans_array() {
        let diag = Diagnostic::warning("W1001", "unreachable code");
        let rendered = render_json(&diag, "src/main.py", "x = 1\n");
        let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed["spans"], serde_json::json!([]));
    }
}
