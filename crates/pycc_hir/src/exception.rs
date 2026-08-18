//! HIR exception-class metadata and handler shape (PEP 3110, #382).

use super::HirStmt;

pub const BUILTIN_EXCEPTION_CLASSES: [&str; 7] = [
    "Exception",
    "ValueError",
    "TypeError",
    "KeyError",
    "IndexError",
    "ZeroDivisionError",
    "RuntimeError",
];

pub fn is_builtin_exception_class(name: &str) -> bool {
    BUILTIN_EXCEPTION_CLASSES.contains(&name)
}

/// Returns the builtin exception class's parent, or `None` for the root and
/// unknown names. The currently supported hierarchy is intentionally flat.
pub fn builtin_exception_parent(name: &str) -> Option<&'static str> {
    match name {
        "Exception" => None,
        name if is_builtin_exception_class(name) => Some("Exception"),
        _ => None,
    }
}

/// A single `except` handler. `exc_type` is `None` for bare `except:` and
/// `name` is the optional `as` binding.
#[derive(Debug, Clone, PartialEq)]
pub struct HirExceptHandler {
    pub exc_type: Option<String>,
    pub name: Option<String>,
    pub body: Vec<HirStmt>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_exception_table_and_flat_parents_are_consistent() {
        for name in BUILTIN_EXCEPTION_CLASSES {
            assert!(is_builtin_exception_class(name));
            let expected = (name != "Exception").then_some("Exception");
            assert_eq!(builtin_exception_parent(name), expected);
        }
        assert!(!is_builtin_exception_class("NotAnException"));
        assert_eq!(builtin_exception_parent("NotAnException"), None);
    }
}
