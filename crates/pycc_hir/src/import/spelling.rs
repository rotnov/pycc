//! The names a foreign import alias may not take because pycc resolves them
//! by their spelling (#1291). Extracted from `import.rs` so the tables stay
//! next to the one predicate that reads them.

/// Whether a foreign alias binding `local_name` would be read as something
/// else (#1291). A name in any of three categories is resolved by its bare
/// spelling somewhere in `pycc_hir` or `pycc_types`, without consulting the
/// import table, so an alias spelled that way would be silently read as the
/// other meaning instead of the CPython module object:
///
/// - a Python builtin ([`PYTHON_BUILTINS`]): `range`, `super`, `property`,
///   `staticmethod`, `classmethod`, the builtin exception classes the module
///   lowering seeds, and the builtin scalar and container annotation names
///   are all matched by spelling;
/// - a `pycc_std` module name (`typing`, `math`, ...), which
///   `expr::std_receiver`'s textual fallback resolves, so
///   `import foo as typing` followed by `if typing.TYPE_CHECKING:` would fold
///   a live body away;
/// - a marker in [`SPELLING_MARKERS`]: the `TYPE_CHECKING` fold, the
///   base-class markers (`crate::typecheck::is_enum_base_name` and its
///   siblings), the decorator and field markers `class::classify_decorator`,
///   `class::classify_class_decorator`, `class::body` and
///   `class::enum_class` match, and the annotation names `func` and
///   `class::attrs` recognise without an import.
///
/// The marker table was derived from the bare-spelling comparisons in
/// `crates/pycc_hir/src` and `crates/pycc_types/src` (non-test files):
/// `git grep -nE 'name\.id\.as_str\(\) == "|as_str\(\) == "[A-Za-z_]+"'`,
/// the string arms of `func::annotation_to_ty` and
/// `func::name_resolves_before_class_defs`, and the three
/// `typecheck::is_*_base_name` helpers. Every hit that is not already a
/// builtin (`range`, `super`, `list`, `dict`, `ExceptionGroup`, ...) or a
/// method name (`__init_subclass__`) or the `__future__` module is listed.
/// The unaliased shapes (`import TYPE_CHECKING`, `import range`,
/// `import Enum`) predate #1291 and are not covered here.
pub(super) fn shadows_a_resolved_spelling(local_name: &str) -> bool {
    PYTHON_BUILTINS.binary_search(&local_name).is_ok()
        || SPELLING_MARKERS.binary_search(&local_name).is_ok()
        || pycc_std::resolve_module(local_name).is_some()
}

/// Every name in CPython 3.14's `dir(builtins)` except the keywords `True`,
/// `False` and `None` (which cannot be an alias), sorted by byte so
/// [`shadows_a_resolved_spelling`] can binary-search it. Regenerate with
/// `python3.14 -c "import builtins, keyword; print(sorted(n for n in
/// dir(builtins) if not keyword.iskeyword(n)))"`.
pub(super) const PYTHON_BUILTINS: &[&str] = &[
    "ArithmeticError",
    "AssertionError",
    "AttributeError",
    "BaseException",
    "BaseExceptionGroup",
    "BlockingIOError",
    "BrokenPipeError",
    "BufferError",
    "BytesWarning",
    "ChildProcessError",
    "ConnectionAbortedError",
    "ConnectionError",
    "ConnectionRefusedError",
    "ConnectionResetError",
    "DeprecationWarning",
    "EOFError",
    "Ellipsis",
    "EncodingWarning",
    "EnvironmentError",
    "Exception",
    "ExceptionGroup",
    "FileExistsError",
    "FileNotFoundError",
    "FloatingPointError",
    "FutureWarning",
    "GeneratorExit",
    "IOError",
    "ImportError",
    "ImportWarning",
    "IndentationError",
    "IndexError",
    "InterruptedError",
    "IsADirectoryError",
    "KeyError",
    "KeyboardInterrupt",
    "LookupError",
    "MemoryError",
    "ModuleNotFoundError",
    "NameError",
    "NotADirectoryError",
    "NotImplemented",
    "NotImplementedError",
    "OSError",
    "OverflowError",
    "PendingDeprecationWarning",
    "PermissionError",
    "ProcessLookupError",
    "PythonFinalizationError",
    "RecursionError",
    "ReferenceError",
    "ResourceWarning",
    "RuntimeError",
    "RuntimeWarning",
    "StopAsyncIteration",
    "StopIteration",
    "SyntaxError",
    "SyntaxWarning",
    "SystemError",
    "SystemExit",
    "TabError",
    "TimeoutError",
    "TypeError",
    "UnboundLocalError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "UnicodeError",
    "UnicodeTranslateError",
    "UnicodeWarning",
    "UserWarning",
    "ValueError",
    "Warning",
    "ZeroDivisionError",
    "_IncompleteInputError",
    "__build_class__",
    "__debug__",
    "__doc__",
    "__import__",
    "__loader__",
    "__name__",
    "__package__",
    "__spec__",
    "abs",
    "aiter",
    "all",
    "anext",
    "any",
    "ascii",
    "bin",
    "bool",
    "breakpoint",
    "bytearray",
    "bytes",
    "callable",
    "chr",
    "classmethod",
    "compile",
    "complex",
    "copyright",
    "credits",
    "delattr",
    "dict",
    "dir",
    "divmod",
    "enumerate",
    "eval",
    "exec",
    "exit",
    "filter",
    "float",
    "format",
    "frozenset",
    "getattr",
    "globals",
    "hasattr",
    "hash",
    "help",
    "hex",
    "id",
    "input",
    "int",
    "isinstance",
    "issubclass",
    "iter",
    "len",
    "license",
    "list",
    "locals",
    "map",
    "max",
    "memoryview",
    "min",
    "next",
    "object",
    "oct",
    "open",
    "ord",
    "pow",
    "print",
    "property",
    "quit",
    "range",
    "repr",
    "reversed",
    "round",
    "set",
    "setattr",
    "slice",
    "sorted",
    "staticmethod",
    "str",
    "sum",
    "super",
    "tuple",
    "type",
    "vars",
    "zip",
];

/// The non-builtin names pycc resolves by their bare spelling, sorted by
/// byte; see [`shadows_a_resolved_spelling`] for how the list was derived.
pub(super) const SPELLING_MARKERS: &[&str] = &[
    "ABC",
    "Annotated",
    "Any",
    "ClassVar",
    "Enum",
    "Final",
    "NDArray",
    "Protocol",
    "Self",
    "StrEnum",
    "TYPE_CHECKING",
    "TypeAlias",
    "abstractmethod",
    "auto",
    "dataclass",
    "dataclass_transform",
    "field",
    "ndarray",
    "override",
    "runtime_checkable",
];

#[cfg(test)]
mod tests {
    use super::{PYTHON_BUILTINS, SPELLING_MARKERS, shadows_a_resolved_spelling};

    /// Both tables must stay strictly sorted by byte, or the binary search
    /// silently misses a name.
    #[test]
    fn both_tables_are_strictly_sorted() {
        for table in [PYTHON_BUILTINS, SPELLING_MARKERS] {
            assert!(table.windows(2).all(|pair| pair[0] < pair[1]), "{table:?}");
        }
    }

    /// The first and last entry of each table, and an ordinary name, so a
    /// broken search at either end is caught.
    #[test]
    fn the_table_ends_are_found_and_an_ordinary_name_is_not() {
        for name in ["ArithmeticError", "zip", "ABC", "runtime_checkable"] {
            assert!(shadows_a_resolved_spelling(name), "{name}");
        }
        assert!(!shadows_a_resolved_spelling("np"));
    }
}
