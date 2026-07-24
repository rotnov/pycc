use pycc_ast::ModModule;
use pycc_diag::{Diagnostic, Span};

pub fn parse(source: &str) -> Result<ModModule, Diagnostic> {
    // ruff_python_parser::parse_module's Err case already covers every
    // syntax-error path: its Ok case is defined as `self.has_valid_syntax()`
    // (i.e. `errors` is empty), never something this function would need to
    // re-check -- verified directly against ruff_python_parser 0.0.6's
    // `Parsed::into_result` source before writing this, not assumed.
    ruff_python_parser::parse_module(source)
        .map(|parsed| parsed.into_syntax())
        .map_err(|e| Diagnostic::error("L0001", e.to_string(), Span::new(0, 0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_function_returning_none_that_calls_print() {
        let module = parse("def main() -> None:\n    print(42)\n").expect("should parse");
        assert_eq!(module.body.len(), 1);
    }

    #[test]
    fn parses_top_level_statements_with_no_main() {
        let module = parse("print(42)\n").expect("should parse");
        assert_eq!(module.body.len(), 1);
    }

    #[test]
    fn syntax_error_becomes_an_l0001_diagnostic() {
        let err = parse("def main(:\n").unwrap_err();
        assert_eq!(err.code, "L0001");
    }
}
