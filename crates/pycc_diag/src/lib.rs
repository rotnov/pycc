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
    pub label: Option<String>,
}

impl Diagnostic {
    pub fn error(
        code: &'static str,
        message: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            span: Some(span),
            label: Some(label.into()),
        }
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            message: message.into(),
            span: None,
            label: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_carries_code_severity_and_span() {
        let d = Diagnostic::error(
            "T0001",
            "argument missing annotation",
            Span::new(10, 14),
            "annotation required",
        );
        assert_eq!(d.code, "T0001");
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.span, Some(Span::new(10, 14)));
        assert_eq!(d.label.as_deref(), Some("annotation required"));
    }

    #[test]
    fn diagnostic_without_span_is_allowed() {
        let d = Diagnostic::warning("W1001", "unreachable code");
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.span, None);
        assert_eq!(d.label, None);
    }
}
