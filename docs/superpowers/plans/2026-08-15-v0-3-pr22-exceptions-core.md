# PR-22 Part 1: Core try/except/finally, raise, builtin exception types

**Issue:** [#540](https://github.com/rotnov/pycc/issues/540) (Part 1 of #382)
**Milestone:** v0.3 — classes & pattern matching
**Design spec:** `docs/superpowers/specs/2026-08-06-v0-3-classes-pattern-matching-design.md` §2
**Decision:** D-172 (supersedes D-005)

## Goal

Implement the core exception handling mechanism: `try`/`except`/`finally`, `raise`, builtin exception types, and exception chains (`raise ... from`). Uses D-172's global exception state + explicit check-and-branch propagation.

## Architecture

### Layer 1: Runtime (`crates/pycc_rt`)

New exception infrastructure in `pycc_rt`:

```rust
// Exception type tags (compile-time known, matching class hierarchy)
pub const EXCEPTION_TYPE_EXCEPTION: u8 = 0;
pub const EXCEPTION_TYPE_VALUE_ERROR: u8 = 1;
pub const EXCEPTION_TYPE_TYPE_ERROR: u8 = 2;
pub const EXCEPTION_TYPE_KEY_ERROR: u8 = 3;
pub const EXCEPTION_TYPE_INDEX_ERROR: u8 = 4;
pub const EXCEPTION_TYPE_ZERO_DIV_ERROR: u8 = 5;
pub const EXCEPTION_TYPE_RUNTIME_ERROR: u8 = 6;

// Exception object (heap-allocated)
pub struct PyExceptionObj {
    pub type_tag: u8,
    pub message: *mut PyStrObj,
    pub cause: *mut PyExceptionObj,   // for `raise ... from ...`
    pub context: *mut PyExceptionObj, // implicit chain during handling
}

// Global exception state
static mut EXCEPTION_ACTIVE: i8 = 0;
static mut EXCEPTION_VALUE: *mut PyExceptionObj = std::ptr::null_mut();
```

Runtime functions:
- `pycc_rt_exception_active() -> i8` — read the flag
- `pycc_rt_exception_value() -> *mut PyExceptionObj` — read the value
- `pycc_rt_exception_clear()` — clear the flag and value
- `pycc_rt_exception_alloc(type_tag: u8, message: *mut PyStrObj) -> *mut PyExceptionObj`
- `pycc_rt_exception_raise(obj: *mut PyExceptionObj)` — set flag + value
- `pycc_rt_exception_raise_with_cause(obj: *mut PyExceptionObj, cause: *mut PyExceptionObj)`
- `pycc_rt_exception_type_matches(obj: *mut PyExceptionObj, type_tag: u8) -> i8` — check if exception matches a type (considering inheritance)
- `pycc_rt_exception_print_and_exit(obj: *mut PyExceptionObj)` — print traceback and exit(1)

Convert existing panic paths to set exception flag:
- `pycc_rt_int_floordiv` / `pycc_rt_int_floormod` / `pycc_rt_int_truediv`: zero divisor → set ZeroDivisionError instead of panicking
- `pycc_rt_dict_get`: missing key → set KeyError instead of panicking
- `pycc_rt_int_list_get`: out of bounds → set IndexError instead of panicking

### Layer 2: HIR (`crates/pycc_hir`)

New HIR statement types:

```rust
pub enum HirStmt {
    // ... existing variants ...
    Try {
        body: Vec<HirStmt>,
        handlers: Vec<HirExceptHandler>,
        orelse: Vec<HirStmt>,    // else block
        finalbody: Vec<HirStmt>, // finally block
    },
    Raise {
        exc: Option<HirExpr>,    // None = bare `raise` (re-raise)
        cause: Option<HirExpr>,  // `raise ... from cause`
    },
}

pub struct HirExceptHandler {
    pub exc_type: Option<String>,  // None = bare `except:` (catch all)
    pub name: Option<String>,      // `except E as e:`
    pub body: Vec<HirStmt>,
}
```

New `Ty` variant for exception types: reuse `Ty::Instance` with the class model from PR-15. Builtin exception classes are pre-declared in the type checker's class registry.

### Layer 3: Type checking (`crates/pycc_types`)

- Register builtin exception classes in the class registry: `Exception` (base), `ValueError`, `TypeError`, `KeyError`, `IndexError`, `ZeroDivisionError`, `RuntimeError` — all inheriting from `Exception`.
- `check_stmt` for `HirStmt::Try`: type-check body, handlers, orelse, finalbody. Handler's `exc_type` must be a subclass of `Exception`. The `as name` binding is available in the handler body.
- `check_stmt` for `HirStmt::Raise`: `exc` must be an instance of `Exception` (or a subclass). `cause` must also be an `Exception` instance.
- Track which functions can raise (a function can raise if it contains a `raise` statement, or calls a function that can raise, or contains an operation that can raise). This information flows to codegen for check-and-branch insertion.
- Definite-assignment tracking through try/except/finally: a variable assigned in `try` is `Maybe` after the try block (the assignment may have been skipped due to an exception).

### Layer 4: MIR (`crates/pycc_mir`)

New MIR statements:

```rust
pub enum MirStmt {
    // ... existing variants ...
    Try {
        body: Vec<MirStmt>,
        handlers: Vec<MirExceptHandler>,
        orelse: Vec<MirStmt>,
        finalbody: Vec<MirStmt>,
    },
    Raise {
        exc_type_tag: u8,
        message: MirExpr,
    },
    RaiseFrom {
        exc_type_tag: u8,
        message: MirExpr,
        cause_type_tag: u8,
        cause_message: MirExpr,
    },
    Reraise,
}

pub struct MirExceptHandler {
    pub exc_type_tag: Option<u8>,  // None = catch all
    pub body: Vec<MirStmt>,
}
```

### Layer 5: Codegen (`crates/pycc_codegen`)

The codegen for `try`/`except`/`finally` uses explicit basic blocks and check-and-branch:

1. **Try body**: emit the body statements. After each potentially-raising operation, insert a check: `if exception_active != 0, branch to handler_chain`.
2. **Handler chain**: for each `except` handler, check if the exception type matches. If yes, clear the flag and run the handler body. If no, check the next handler. If no handler matches, run the finally block and re-raise (propagate).
3. **Else block**: runs if no exception was raised in the try body.
4. **Finally block**: runs in all exit paths (normal, exception caught, exception not caught). Implemented as a shared basic block that all exit paths branch to.
5. **Raise**: allocate exception object, set global flag/value, jump to the current function's exception-exit block (or the nearest enclosing try's handler chain).

For functions that can raise but don't have a try block: after each potentially-raising operation, check the flag and return early (propagating the exception to the caller).

## Implementation tasks

1. **Runtime**: Add exception object, global state, and runtime functions to `pycc_rt`. Convert existing panic paths to set exception flag.
2. **HIR**: Add `Try`, `Raise` to `HirStmt`. Add `HirExceptHandler`. Update `lower_stmt` to handle `Stmt::Try` and `Stmt::Raise`.
3. **Type checking**: Register builtin exception classes. Add `check_stmt` arms for `Try` and `Raise`. Track raise-capability for functions.
4. **MIR**: Add `Try`, `Raise`, `RaiseFrom`, `Reraise` to `MirStmt`. Update `lower_stmt`.
5. **Codegen**: Implement try/except/finally codegen with check-and-branch. Implement raise codegen. Insert exception checks after potentially-raising operations.
6. **Tests**: Unit tests for each layer. Integration tests in `tests/issue_382_exceptions.rs`. Conformance fixture for PEP 409.
7. **Documentation**: Update `docs/RUNTIME.md`, `docs/DIAGNOSTICS.md`, `docs/ROADMAP.md`, `PYTHON_STANDARDS.md`.

## Scope cuts for v0.3 thin slice

- Only builtin exception types (no custom exception classes yet — Part 2).
- Exception messages are strings only (no arbitrary arguments).
- No `except*` / ExceptionGroup (Part 3).
- No `OSError` hierarchy, no `finally` restrictions, no `except A, B:` (Part 4).
- `raise` with no argument (re-raise) only valid inside an except handler.
- `else` block in try/except/else is supported.
