//! Enum-loop unrolling unit tests for the type-checking crate root.
//!
//! Extracted verbatim from `tests/exception_handling.rs` under AGENTS.md's
//! decomposability rule (part of #695, which tracks decomposing the oversized
//! `tests.rs` these child modules came out of). These are the tests that
//! exercise `unroll_enum_loops` recursing through ordinary statement nesting
//! — functions, `if`, `while`, and `for` — rather than through the constructs
//! covered by the `exception_handling` child module. Their shared helper
//! (`parse_check_resolve`) stays in the parent, because tests that remain
//! there call it too and a parent cannot see a child's private items. As a
//! child module this still sees the parent's private items directly through
//! `use super::*`, so nothing needed widened visibility; only the tests'
//! location changed.

use super::*;

// -- enum-loop unrolling through ordinary statement nesting -------------

#[test]
fn enum_loop_inside_function_unrolls() {
    // Covers the `HirItem::Function` arm of `unroll_enum_loops` —
    // a function body containing an enum loop is unrolled.
    let src = "\
from enum import Enum
class Color(Enum):
    RED = 1
    GREEN = 2
def f() -> int:
    x = 0
    for c in Color:
        x = x + 1
    return x
f()
";
    parse_check_resolve(src).expect("check should succeed");
}

#[test]
fn enum_loop_with_nested_if_unrolls() {
    // Covers the `HirStmt::If` arm of `unroll_enum_loops_in_stmts` —
    // an `if` inside an enum loop body is recursed into.
    let src = "\
from enum import Enum
class Color(Enum):
    RED = 1
    GREEN = 2
x = 0
for c in Color:
    if x > 0:
        x = x + 1
";
    parse_check_resolve(src).expect("check should succeed");
}

#[test]
fn enum_loop_with_nested_while_unrolls() {
    // Covers the `HirStmt::While` arm of `unroll_enum_loops_in_stmts`.
    let src = "\
from enum import Enum
class Color(Enum):
    RED = 1
    GREEN = 2
x = 0
for c in Color:
    while x < 10:
        x = x + 1
";
    parse_check_resolve(src).expect("check should succeed");
}

#[test]
fn enum_loop_with_nested_for_range_unrolls() {
    // Covers the `HirStmt::ForRange` arm of `unroll_enum_loops_in_stmts`.
    let src = "\
from enum import Enum
class Color(Enum):
    RED = 1
    GREEN = 2
x = 0
for c in Color:
    for i in range(3):
        x = x + 1
";
    parse_check_resolve(src).expect("check should succeed");
}

#[test]
fn plain_if_while_for_range_recurse_when_module_has_an_enum() {
    // `unroll_enum_loops` runs `unroll_enum_loops_in_stmts` over every
    // top-level statement whenever *any* class in the module is an enum --
    // not only over statements that structurally contain a `for ... in
    // <EnumClass>` loop. `enum_loop_with_nested_if_unrolls` (and its
    // `_while`/`_for_range` siblings above) nest the `if`/`while`/`for
    // range` *inside* the enum loop's own body, which the `ForList` enum
    // branch clones verbatim rather than recursing into -- so those tests
    // never actually reach `unroll_enum_loops_in_stmts`'s own
    // `HirStmt::If`/`HirStmt::While`/`HirStmt::ForRange` arms. Reach them
    // here directly with top-level `if`/`while`/`for i in range(..)`
    // statements that sit alongside an enum class definition but are not
    // themselves nested inside any enum loop.
    let src = "\
from enum import Enum
class Color(Enum):
    RED = 1
    GREEN = 2
x = 0
if x > 0:
    x = x + 1
while x < 10:
    x = x + 1
for i in range(3):
    x = x + 1
";
    parse_check_resolve(src).expect("check should succeed");
}
