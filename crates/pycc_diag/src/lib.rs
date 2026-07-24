use unicode_width::UnicodeWidthChar;

const TAB_STOP: usize = 4;

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

    pub fn render_human(&self, path: &str, source: &str) -> String {
        let level = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let header = format!("{level}[{}]: {}", self.code, self.message);
        let Some(span) = self.span else {
            return header;
        };

        let start = span.start as usize;
        let end = span.end as usize;
        let before = &source[..start];
        let line_start = before.rfind('\n').map_or(0, |index| index + 1);
        let line_end = source[start..]
            .find('\n')
            .map_or(source.len(), |index| start + index);
        let line_number = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let source_prefix = &source[line_start..start];
        let indent_width = display_width(source_prefix, 0);
        let column = indent_width + 1;
        let caret_width = display_width(&source[start..end.min(line_end)], indent_width).max(1);
        let normalized_path = path.replace('\\', "/");
        let display_line_end = if source.as_bytes().get(line_end.wrapping_sub(1)) == Some(&b'\r') {
            line_end - 1
        } else {
            line_end
        };
        let source_line = expand_tabs(&source[line_start..display_line_end]);
        let gutter_width = line_number.to_string().len();

        format!(
            "{header}\n --> {normalized_path}:{line_number}:{column}\n\
             {blank:>gutter_width$} |\n\
             {line_number:>gutter_width$} | {source_line}\n\
             {blank:>gutter_width$} | {indent}{carets} {label}",
            blank = "",
            indent = " ".repeat(indent_width),
            carets = "^".repeat(caret_width),
            label = self.message,
        )
    }
}

fn display_width(text: &str, initial_width: usize) -> usize {
    text.chars().fold(0, |width, character| {
        if character == '\t' {
            let absolute_width = initial_width + width;
            width + (TAB_STOP - absolute_width % TAB_STOP)
        } else {
            width + UnicodeWidthChar::width(character).unwrap_or(0)
        }
    })
}

fn expand_tabs(text: &str) -> String {
    let mut expanded = String::with_capacity(text.len());
    let mut width = 0;
    for character in text.chars() {
        if character == '\t' {
            let spaces = TAB_STOP - width % TAB_STOP;
            expanded.extend(std::iter::repeat_n(' ', spaces));
            width += spaces;
        } else {
            expanded.push(character);
            width += UnicodeWidthChar::width(character).unwrap_or(0);
        }
    }
    expanded
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
        assert_eq!(
            d.render_human("input.py", ""),
            "warning[W1001]: unreachable code"
        );
    }

    #[test]
    fn renders_a_primary_span_with_a_label() {
        let d = Diagnostic::error("L0003", "assignment is unsupported", Span::new(0, 7));
        assert_eq!(
            d.render_human(r"src\input.py", "x = 1\ny"),
            concat!(
                "error[L0003]: assignment is unsupported\n",
                " --> src/input.py:1:1\n",
                "  |\n",
                "1 | x = 1\n",
                "  | ^^^^^ assignment is unsupported",
            )
        );
    }

    #[test]
    fn renders_a_zero_width_span_on_a_later_final_line() {
        let d = Diagnostic::error("L0001", "expected expression", Span::new(3, 3));
        assert_eq!(
            d.render_human("input.py", "x\n\né"),
            concat!(
                "error[L0001]: expected expression\n",
                " --> input.py:3:1\n",
                "  |\n",
                "3 | é\n",
                "  | ^ expected expression",
            )
        );
    }

    #[test]
    fn expands_tabs_and_accounts_for_wide_characters_before_the_caret() {
        let source = "\t界x\n";
        let start = "\t界".len() as u32;
        let d = Diagnostic::error("L0003", "unsupported name", Span::new(start, start + 1));
        assert_eq!(
            d.render_human("input.py", source),
            concat!(
                "error[L0003]: unsupported name\n",
                " --> input.py:1:7\n",
                "  |\n",
                "1 |     界x\n",
                "  |       ^ unsupported name",
            )
        );
    }

    #[test]
    fn strips_carriage_returns_from_crlf_source_lines() {
        let source = "x\r\n\t界y\r\n";
        let start = source.find('y').unwrap() as u32;
        let d = Diagnostic::error("L0003", "unsupported name", Span::new(start, start + 1));
        assert_eq!(
            d.render_human("input.py", source),
            concat!(
                "error[L0003]: unsupported name\n",
                " --> input.py:2:7\n",
                "  |\n",
                "2 |     界y\n",
                "  |       ^ unsupported name",
            )
        );
    }
}
