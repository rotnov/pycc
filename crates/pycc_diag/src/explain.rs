//! `pycc explain <code>`'s content source (D-150): a hand-authored `const`
//! slice of every code registered in `docs/DIAGNOSTICS.md`'s "Initial
//! registry" table, linear-scanned by [`find`] -- the same shape
//! `pycc_std::REGISTRY`/`resolve_symbol` already established for a
//! structurally identical "small hand-authored table, looked up by exact
//! string match" problem (D-136). See D-150 for why this is a `const`
//! slice rather than a runtime `docs/DIAGNOSTICS.md` parser, and for the
//! completeness test's drift-guard mechanism (`tests` module below).
//!
//! Every code registered in `docs/DIAGNOSTICS.md` must have exactly one
//! matching entry here, with the same severity (after this test's
//! `warning→error`-style normalization -- see `tests::registry_severity`
//! below). Adding a new code to that table without a matching entry here
//! fails `completeness_test_covers_every_registered_code_and_severity` in
//! CI; this is deliberate, not a bug in the test.

use crate::Severity;

/// One `docs/DIAGNOSTICS.md` registry entry's long-form documentation:
/// what `pycc explain <code>` prints. `summary` is a short, single-line
/// restatement of the registry table's own "Message (short form)" column;
/// `explanation` is free-form prose describing the real trigger condition
/// (grounded in the code's actual emission site(s), not just the registry
/// table's terse summary -- see D-150); `example` is a minimal Python
/// snippet showing the shape of code the rule is about.
#[derive(Debug, Clone, Copy)]
pub struct DiagnosticExplanation {
    pub code: &'static str,
    pub severity: Severity,
    pub summary: &'static str,
    pub explanation: &'static str,
    pub example: &'static str,
}

/// The full set of documented diagnostic codes (D-150). Kept in
/// `docs/DIAGNOSTICS.md`'s own table order for easy side-by-side review,
/// not that order has any runtime significance -- [`find`] does a plain
/// linear scan regardless of position.
pub const EXPLANATIONS: &[DiagnosticExplanation] = &[
    DiagnosticExplanation {
        code: "C0001",
        severity: Severity::Error,
        summary: "valid Python construct is not implemented by this pycc version",
        explanation: "\
C0001 is a versioned capability diagnostic, not a rejected-by-design language \
rule: it fires whenever HIR lowering reaches a syntactically valid Python \
statement, expression, or annotation shape that this pycc version's frontend \
does not yet lower -- a class definition, a `try`/`except` block, a `with` \
statement, an unrecognized import shape, or a type annotation more complex \
than a bare name, for example. The construct remains reserved and stops \
producing C0001 the moment the corresponding roadmap slice is implemented; \
until then the diagnostic's span points at the unsupported node so the \
message stays actionable rather than a bare \"not implemented\".",
        example: "\
class Point:
    def __init__(self, x: int, y: int) -> None:
        self.x = x
        self.y = y
",
    },
    DiagnosticExplanation {
        code: "C0002",
        severity: Severity::Error,
        summary: "recognized stdlib module, but the specific imported symbol is not registered",
        explanation: "\
C0002 is distinct from C0001 (D-136): it fires when a `from <module> import \
<name>` statement's module *is* recognized by pycc_std's hand-authored \
registry (currently only `math`), but the specific imported name inside it \
is not one of the registered symbols (currently only `sqrt` and `pi`). This \
distinguishes \"we don't support this import shape at all\" (C0001) from \"we \
support this module, just not this particular symbol yet\" (C0002), and it \
fails the whole import statement rather than partially binding the names \
that did resolve.",
        example: "\
from math import isnan
",
    },
    DiagnosticExplanation {
        code: "L0001",
        severity: Severity::Error,
        summary: "syntax error (span + expected set)",
        explanation: "\
L0001 is pycc's parser-level syntax-error code: the parser (via \
ruff_python_parser) could not build a valid syntax tree from the source, and \
the diagnostic's span and message come directly from the underlying parse \
error. L0001 is also reused, without a parser \"expected set\", for a \
post-parse context violation caught during HIR lowering rather than \
parsing: `break`/`continue` outside a loop, `async for` outside an async \
function, and `yield`/`yield from` outside a function -- CPython classifies \
all of these as `SyntaxError` too, even though pycc only detects them one \
lowering stage later than the grammar itself. `T0024` (\"`return` outside a \
function\") is the closest existing precedent for this same \"keyword valid \
only in a specific context\" family but deliberately stayed under `T0xxx` \
instead, an accepted, unfixed inconsistency.",
        example: "\
def f(:
    pass
",
    },
    DiagnosticExplanation {
        code: "L0002",
        severity: Severity::Error,
        summary: "Python version mismatch (feature needs 3.14 level)",
        explanation: "\
L0002 is reserved for a source construct that requires a Python grammar or \
runtime feature beyond the language level this pycc version's frontend \
targets (3.14, D-012). It is not currently emitted: pycc's parser does not \
yet implement any version-gated grammar dispatch, so there is nothing in \
today's frontend that can distinguish \"valid 3.14 syntax\" from \"syntax that \
needs a later Python version\" -- every construct outside the currently \
implemented subset falls through to C0001 instead. Do not confuse this with \
`pycc.toml`'s separate `[project] python = \"...\"` validation (D-012, \
`src/project_config.rs`), which rejects any value other than the literal \
string `\"3.14\"` with a plain configuration error, not this diagnostic code.",
        example: "\
# a hypothetical future syntax feature beyond Python 3.14 would be
# reported here once L0002 is wired up to a real version check
",
    },
    DiagnosticExplanation {
        code: "T0001",
        severity: Severity::Error,
        summary: "public function missing annotation",
        explanation: "\
A function without a leading underscore is part of a module's public \
surface. pycc's type checker infers types only for private \
(underscore-prefixed) helper functions; every public function must declare \
both its parameter types and its return type explicitly, since the checker \
never performs whole-program inference across a module boundary it cannot \
see into. T0001 fires separately for a missing parameter annotation \
(\"parameter `<name>` of public function `<fn>` needs a type annotation\") \
and a missing return annotation (\"public function `<fn>` needs a return \
type annotation\") -- a function can trigger it more than once if both are \
missing.",
        example: "\
def add(a: int, b: int) -> int:
    return a + b
",
    },
    DiagnosticExplanation {
        code: "T0002",
        severity: Severity::Error,
        summary: "`Any` outside interop boundary",
        explanation: "\
The bare name `Any` is rejected as a type annotation everywhere in pycc \
code today: \"`Any` is not permitted in pycc code outside a declared interop \
boundary\" is the literal message HIR lowering's annotation resolver \
produces. pycc's static, whole-function type checking has no way to make \
`Any` sound without an explicit escape hatch, and that escape hatch (a \
declared CPython interop boundary) is part of the planned v0.7 transparent \
interop work, not yet implemented -- so for now, every occurrence of `Any` \
in an annotation is rejected outright, regardless of position.",
        example: "\
from typing import Any

def f(x: Any) -> int:
    return 0
",
    },
    DiagnosticExplanation {
        code: "T0003",
        severity: Severity::Error,
        summary: "untyped empty container needs annotation",
        explanation: "\
T0003 is reserved for an empty list/dict/set literal (`[]`, `{}`, `set()`) \
assigned to a name with no surrounding annotation to establish its element \
type -- CPython accepts `x = []` and infers nothing about future element \
types, but pycc's static checker needs a concrete element type before it \
can type-check any later use of `x`. It is not currently emitted: pycc's \
frontend does not yet reach an empty-container-literal code path that would \
raise it (empty container literals are documented as future work; the \
registry entry exists to reserve this code's meaning ahead of that \
implementation).",
        example: "\
def f() -> None:
    x = []  # element type not yet inferable from context
",
    },
    DiagnosticExplanation {
        code: "T0021",
        severity: Severity::Error,
        summary: "name resolution (including an unbound local), operand, call, or inference type mismatch",
        explanation: "\
T0021 is the type checker's broadest, most frequently emitted code: it \
covers name resolution (\"name `<x>` is not defined\", or \"local name `<x>` \
is not bound before this use\" for a name that is assigned somewhere in the \
function but not yet on this path), calling a non-callable binding, \
operator/operand type mismatches inside expression inference, and general \
call-argument type mismatches -- including the stdlib-symbol shapes D-136 \
adds (calling a stdlib constant, or referencing a stdlib function without \
calling it). Different call sites across `pycc_types` construct T0021 with \
different messages for these distinct situations; the shared code reflects \
that they are all instances of the same underlying category (a name or \
value did not have the type or binding shape an expression needed), not \
that pycc considers them identical errors.",
        example: "\
def f() -> int:
    return y  # `y` is never defined
",
    },
    DiagnosticExplanation {
        code: "T0022",
        severity: Severity::Error,
        summary: "return type mismatch",
        explanation: "\
T0022 fires when a function's `return` statement's value has a type that is \
not assignable to the function's own declared (or, for a private helper, \
inferred) return type -- including the implicit `None` return type when a \
function's body falls off the end without an explicit `return` in a path \
that needs one.",
        example: "\
def f() -> int:
    return \"not an int\"
",
    },
    DiagnosticExplanation {
        code: "T0023",
        severity: Severity::Error,
        summary: "incompatible assignment",
        explanation: "\
T0023 fires when a plain (non-annotated) reassignment's value type is not \
assignable to the type the target name was already bound to from an earlier \
assignment -- e.g. rebinding a name first bound to `int` with a `str` value \
later in the same function. This is distinct from `T0025` (an annotated \
assignment's initializer disagreeing with its own declared annotation) and \
`T0026` (an assignment disagreeing with an earlier value-less `x: <type>` \
declaration): T0023 is specifically \"this name was already inferred to \
have a type from a real value, and this new value doesn't fit it\".",
        example: "\
def f() -> None:
    x = 1
    x = \"now a string\"
",
    },
    DiagnosticExplanation {
        code: "T0024",
        severity: Severity::Error,
        summary: "`return` outside a function",
        explanation: "\
T0024 fires when a `return` statement appears outside any function body -- \
at module (top) level, for example. CPython raises `SyntaxError` for this \
too, but pycc catches it during type checking rather than parsing, so it \
keeps its own `T0xxx` code instead of joining `L0001`'s reused \
context-violation family (`break`/`continue`/`async for`/`yield` outside \
their valid context); this is a deliberate, accepted inconsistency, not an \
oversight.",
        example: "\
return 1
",
    },
    DiagnosticExplanation {
        code: "T0025",
        severity: Severity::Error,
        summary: "annotated-assignment initializer incompatible with its declared annotation",
        explanation: "\
T0025 fires when an annotated assignment's own right-hand-side value has a \
type that does not match the annotation written on the same statement, \
e.g. `x: int = \"hello\"`. The annotation itself is what determines `x`'s \
type going forward (not the initializer's inferred type) -- T0025 is the \
check that the initializer is honest about that annotation at the point it \
is declared.",
        example: "\
def f() -> None:
    x: int = \"hello\"
",
    },
    DiagnosticExplanation {
        code: "T0026",
        severity: Severity::Error,
        summary: "a later plain or annotated assignment is incompatible with an earlier value-less declaration",
        explanation: "\
T0026 fires when a name was first declared with a value-less annotation \
(`x: int`, with no `= ...`) and a later assignment or re-declaration to that \
same name has a type that is not assignable to the original declaration. \
This is distinct from `T0023` (nothing was previously \"inferred\" here -- \
the name was declared, never assigned, before this point) and `T0025` (the \
mismatch is against an earlier statement's declaration, not the current \
statement's own annotation).",
        example: "\
def f() -> None:
    x: int
    x = \"not an int\"
",
    },
    DiagnosticExplanation {
        code: "T0030",
        severity: Severity::Error,
        summary: "non-exhaustive `match` (missing cases listed)",
        explanation: "\
T0030 is reserved for a `match` statement (PEP 634 structural pattern \
matching) whose patterns do not cover every possible value of the matched \
expression's type, with the missing cases listed in the diagnostic. It is \
not currently emitted: pycc's parser and HIR lowering do not implement \
`match` statements at all yet (PEP 634-636 support is unchecked in \
`docs/PYTHON_STANDARDS.md`'s own conformance table), so there is no \
exhaustiveness check to run. The registry entry reserves this code's \
meaning ahead of that implementation.",
        example: "\
def describe(n: int) -> str:
    match n:
        case 0:
            return \"zero\"
        # missing a case (or a wildcard `case _:`) for every other int
",
    },
    DiagnosticExplanation {
        code: "T0031",
        severity: Severity::Error,
        summary: "`@override` without matching base method",
        explanation: "\
T0031 is reserved for a method decorated `@override` (PEP 698, `typing.\
override`) that does not actually override a same-named method on any base \
class. It is not currently emitted: pycc has no class/inheritance support \
in its frontend yet (PEP 698 is unchecked in `docs/PYTHON_STANDARDS.md`'s \
own conformance table), so neither `@override` nor the base-class lookup it \
depends on exist to check. The registry entry reserves this code's meaning \
ahead of class support landing.",
        example: "\
class Base:
    def run(self) -> None: ...

class Derived(Base):
    @override
    def run_typo(self) -> None: ...  # does not override anything on Base
",
    },
    DiagnosticExplanation {
        code: "T0032",
        severity: Severity::Error,
        summary: "list literal elements must all share one type",
        explanation: "\
T0032 fires when a `list[...]` literal's elements do not all infer to the \
same element type, e.g. mixing `int` and `str` values in one `[...]` \
literal. pycc's v0.2 list representation needs one concrete, uniform \
element type for the whole list (D-105); a genuinely heterogeneous list has \
no such type to assign it.",
        example: "\
def f() -> None:
    xs = [1, \"two\", 3]
",
    },
    DiagnosticExplanation {
        code: "T0033",
        severity: Severity::Error,
        summary: "value does not support list/dict/set operations, or `len()` called with the wrong number of arguments",
        explanation: "\
T0033 covers a family of \"this value does not support this container \
operation\" mismatches: subscript (`x[i]`), slicing, item assignment \
(`x[i] = v`), iterating with `for`, and the built-in container methods \
`.append()`/`.pop()`/`.get()`/`.add()`, when the receiver's inferred type \
isn't a `list`/`dict`/`set` at all, or isn't the specific container kind \
that method requires. It also covers `len(...)` being called with a number \
of arguments other than exactly one, and `len(...)`'s single argument not \
being a `list[T]`/`dict[K, V]`/`set[T]`.",
        example: "\
def f() -> int:
    x = 5
    return len(x, x)  # len() takes exactly one argument
",
    },
    DiagnosticExplanation {
        code: "T0034",
        severity: Severity::Error,
        summary: "list codegen only supports `list[int]` in v0.2",
        explanation: "\
T0034 fires when a `list[T]` value's element type `T` is anything other \
than `int` -- pycc's v0.2 list codegen (D-105) implements only \
`list[int]`'s runtime representation; `list[str]`, `list[bool]`, nested \
lists, and every other element type are recognized by the type checker \
(so `T0032`'s uniform-element-type check still applies to them) but not \
yet compilable, and are rejected here instead of silently miscompiled.",
        example: "\
def f() -> None:
    xs: list[str] = [\"a\", \"b\"]
",
    },
    DiagnosticExplanation {
        code: "T0035",
        severity: Severity::Error,
        summary: "dict literal key/value pairs must all share one key type and one value type",
        explanation: "\
T0035 fires when a `dict[...]` literal's entries do not all infer to one \
consistent key type and one consistent value type, mirroring `T0032`'s \
uniform-element-type rule for lists -- e.g. mixing a `str` key with an \
`int` key in the same `{...}` literal, or mixing value types across \
entries.",
        example: "\
def f() -> None:
    d = {\"a\": 1, 2: \"b\"}
",
    },
    DiagnosticExplanation {
        code: "T0036",
        severity: Severity::Error,
        summary: "dict codegen only supports `dict[str, int]` in v0.2",
        explanation: "\
T0036 fires when a `dict[K, V]` value's key/value types are anything other \
than exactly `str` keys and `int` values -- pycc's v0.2 dict codegen \
(D-122) implements only `dict[str, int]`'s dense insertion-ordered \
representation; other key/value type combinations are type-checked but not \
yet compilable and are rejected here rather than silently miscompiled.",
        example: "\
def f() -> None:
    d: dict[int, int] = {1: 1, 2: 2}
",
    },
    DiagnosticExplanation {
        code: "T0037",
        severity: Severity::Error,
        summary: "set literal elements must all share one type",
        explanation: "\
T0037 fires when a `set(...)`/`{...}` set literal's elements do not all \
infer to the same element type, mirroring `T0032`'s uniform-element-type \
rule for lists.",
        example: "\
def f() -> None:
    xs = {1, \"two\", 3}
",
    },
    DiagnosticExplanation {
        code: "T0038",
        severity: Severity::Error,
        summary: "set codegen only supports `set[int]` in v0.2",
        explanation: "\
T0038 fires when a `set[T]` value's element type `T` is anything other than \
`int` -- pycc's v0.2 set codegen (D-122) implements only `set[int]`'s \
representation; other element types are type-checked but not yet \
compilable and are rejected here rather than silently miscompiled.",
        example: "\
def f() -> None:
    xs: set[str] = {\"a\", \"b\"}
",
    },
    DiagnosticExplanation {
        code: "T0039",
        severity: Severity::Error,
        summary: "tuple element type not compiled yet (int/bool/float only, D-116)",
        explanation: "\
T0039 fires when a `tuple[...]` value contains an element type other than \
`int`, `bool`, or `float` -- pycc's v0.2 tuple codegen (D-116) represents a \
tuple as a fixed SSA struct value whose fields are scalar types only; \
`str`, nested containers, and other element shapes inside a tuple are not \
yet compilable and are rejected here.",
        example: "\
def f() -> None:
    t: tuple[str, int] = (\"a\", 1)
",
    },
    DiagnosticExplanation {
        code: "T0040",
        severity: Severity::Error,
        summary: "tuple index must be a non-negative literal integer within range",
        explanation: "\
T0040 fires on a tuple-index read (`t[i]`) whose index is not a literal, \
non-negative integer known at compile time, or whose value falls outside \
the tuple's own fixed length -- pycc's v0.2 tuple representation (D-116) \
supports only literal-index reads (no dynamic indexing, no negative \
indices) because each field access lowers directly to a struct-field read.",
        example: "\
def f() -> int:
    t: tuple[int, int] = (1, 2)
    return t[5]  # only indices 0 and 1 exist
",
    },
    DiagnosticExplanation {
        code: "T0041",
        severity: Severity::Error,
        summary: "local name may not be bound on every path reaching this use",
        explanation: "\
T0041 fires when a local name is assigned on only some of the control-flow \
paths that can reach a later read of it -- e.g. inside an `if` with no \
`else`, or inside a `while`/`for` body that might run zero times -- rather \
than on every path (issue #118 Part 1). This is pycc's strict AOT \
equivalent of CPython's `UnboundLocalError`, but caught at compile time via \
a three-state binding model (definitely bound / maybe bound / unbound) \
tracked across `if`/`while`/`for` control-flow joins, instead of at \
runtime. T0041 is distinct from `T0021`'s \"local name `<name>` is not \
bound before this use\" message: T0021 covers a name never assigned on any \
path reaching the read, while T0041 covers a name assigned on some but not \
all paths.",
        example: "\
def read_value(flag: bool) -> int:
    if flag:
        x = 1
    return x
",
    },
    DiagnosticExplanation {
        code: "T0042",
        severity: Severity::Error,
        summary: "generic-function shape or call-site instantiation rejected beyond the v0.2 thin slice",
        explanation: "\
T0042 covers every way a PEP 695 generic function or `type` alias can fall \
outside pycc's v0.2 thin generics slice (D-133/D-134/D-135): more than one \
type parameter on a single function; a type parameter used in a container \
position (e.g. `list[T]`); a call site whose different occurrences of the \
same type parameter resolve to different concrete types; a call site whose \
argument for the type parameter is not `int`/`float`/`bool`/`str`; a \
generic function's body calling itself or any other generic function \
(recursive generic instantiation is outside the single-call-site \
monomorphization this slice implements); and an argument whose type cannot \
be inferred because it depends on a still-uninstantiated generic parameter.",
        example: "\
def first(x: T, y: T) -> T:
    return x

def use() -> None:
    first(1, \"mismatched\")  # both occurrences of T must resolve the same
",
    },
    DiagnosticExplanation {
        code: "T0043",
        severity: Severity::Error,
        summary: "attribute access, attribute assignment, or method call on a non-instance value",
        explanation: "\
T0043 fires when `base.attr`, `base.attr = value`, or `base.method(...)` is \
used against a `base` whose type is not a class instance (D-154, Part 1 of \
#375) -- e.g. an `int`, a `list[int]`, or any other non-`Ty::Instance` \
value. pycc's v0.3 class model resolves attribute and method access \
entirely at compile time against the base's declared class shape, so the \
base must be known, structurally, to actually be an instance of some class \
before an attribute or method name on it can mean anything.",
        example: "\
def f(x: int) -> int:
    return x.value  # `int` is not a class instance
",
    },
    DiagnosticExplanation {
        code: "T0044",
        severity: Severity::Error,
        summary: "class has no attribute or method with the given name",
        explanation: "\
T0044 fires when `base.attr`, `base.attr = value`, or `base.method(...)` \
names an attribute or method that the base's own class does not declare \
(D-154, Part 1 of #375) -- an instance attribute is only ever established \
by a first assignment inside `__init__`, and a method is only ever one of \
the class body's own `def`s, so any other name is unknown at compile time. \
A mismatched argument count or type at an instantiation or method call site \
is a separate condition and reuses `T0021` instead, exactly like an \
ordinary function call.",
        example: "\
class Point:
    def __init__(self, x: int) -> None:
        self.x = x

def f(p: Point) -> int:
    return p.y  # `Point` has no attribute named `y`
",
    },
    DiagnosticExplanation {
        code: "O0201",
        severity: Severity::Error,
        summary: "value used after move across scope boundary",
        explanation: "\
O0201 is an internal-only diagnostic: legal Python source never triggers it \
in practice, because pycc's move-semantics optimization (uniqueness \
analysis, MEMORY_OWNERSHIP.md) is designed to be semantics-preserving -- \
every optimization it performs is provably safe for the program it was \
applied to. The code exists for the compiler's own internal assertions and \
`--emit mir` tooling output; if it is ever observed firing on real, legal \
user code, that is a pycc bug (auto-reported), not a defect in the user's \
program. There is currently no code path in this pycc version that emits \
O0201 at all -- the ownership/move-tracking pass it would guard has not \
landed yet.",
        example: "\
# O0201 has no user-triggerable source today -- it exists to guard pycc's
# own internal move-semantics invariants, not to reject Python source.
",
    },
    DiagnosticExplanation {
        code: "O0301",
        severity: Severity::Error,
        summary: "non-Shareable value crosses thread boundary without move",
        explanation: "\
O0301 is reserved for pycc's planned thread-safety ownership check \
(MEMORY_OWNERSHIP.md): a value crossing a thread boundary -- as a thread \
target argument, captured by a closure a thread runs, or as a \
`queue.Queue[T]` payload -- must either be `Shareable` (deeply immutable: \
`int`, `str`, a frozen dataclass, a tuple of `Shareable` values, ...) or be \
provably moved (the sender loses access, verified by uniqueness analysis); \
otherwise it is rejected with O0301. It is not currently emitted: pycc has \
no threading support in its frontend yet, so there is no thread-boundary \
crossing for this check to run against.",
        example: "\
# O0301 is reserved for pycc's not-yet-implemented threading support;
# no current pycc source can reach this check.
",
    },
    DiagnosticExplanation {
        code: "O0302",
        severity: Severity::Error,
        summary: "lock-guarded field accessed without holding lock",
        explanation: "\
O0302 is reserved for pycc's planned lock-discipline check \
(MEMORY_OWNERSHIP.md): shared mutable state guarded by a `threading.Lock` \
gets checked lock discipline, and an access to a lock-guarded field without \
provably holding the corresponding lock is rejected with O0302. It is not \
currently emitted: pycc has no threading or lock-discipline support in its \
frontend yet, so there is no guarded-field access for this check to run \
against.",
        example: "\
# O0302 is reserved for pycc's not-yet-implemented lock-discipline check;
# no current pycc source can reach this check.
",
    },
    DiagnosticExplanation {
        code: "E0100",
        severity: Severity::Error,
        summary: "`eval`/`exec`/`compile` not compilable",
        explanation: "\
E0100 is reserved for a call to `eval`, `exec`, or `compile` -- constructs \
that execute Python source discovered at runtime, which is fundamentally \
incompatible with pycc's ahead-of-time compilation model (there is no \
running interpreter to hand dynamically-constructed code to). It is not \
currently emitted: this is part of the planned v0.7 dynamic-Python \
rejection surface (`--interop-policy`), not yet implemented, so calls to \
these names fall through to the generic `C0001` \"not implemented yet\" \
path today instead.",
        example: "\
def f() -> None:
    eval(\"1 + 1\")
",
    },
    DiagnosticExplanation {
        code: "E0101",
        severity: Severity::Error,
        summary: "monkey-patching foreign class/module",
        explanation: "\
E0101 is reserved for code that reassigns an attribute on a class or module \
pycc did not itself define (monkey-patching) -- a pattern pycc's static, \
whole-program-unaware compilation model cannot support, since it assumes a \
type's method set is fixed at compile time. It is not currently emitted: \
this is part of the planned v0.7 dynamic-Python rejection surface, not yet \
implemented; pycc's frontend does not yet support class definitions at all, \
so there is nothing to monkey-patch today regardless.",
        example: "\
import math

math.sqrt = lambda x: x  # reassigning an attribute on a foreign module
",
    },
    DiagnosticExplanation {
        code: "E0102",
        severity: Severity::Error,
        summary: "dynamic attribute injection",
        explanation: "\
E0102 is reserved for code that adds a new attribute to an object outside \
its class's declared shape at runtime (e.g. via `setattr` with a \
non-literal name, or assigning to an attribute a `__slots__`-less class \
never declared) -- incompatible with pycc's fixed, compile-time-known \
object layout. It is not currently emitted: this is part of the planned \
v0.7 dynamic-Python rejection surface, not yet implemented; pycc has no \
class support yet, so there is no object layout for this check to guard.",
        example: "\
def f(obj: object) -> None:
    setattr(obj, some_dynamic_name, 1)
",
    },
    DiagnosticExplanation {
        code: "E0103",
        severity: Severity::Error,
        summary: "dynamic `type()` class creation",
        explanation: "\
E0103 is reserved for the three-argument `type(name, bases, namespace)` \
call form that constructs a new class object at runtime -- incompatible \
with pycc's ahead-of-time, statically-known class model. It is not \
currently emitted: this is part of the planned v0.7 dynamic-Python \
rejection surface, not yet implemented; pycc has no class support yet, so \
this check has nothing to guard today.",
        example: "\
Point = type(\"Point\", (), {})
",
    },
    DiagnosticExplanation {
        code: "E0104",
        severity: Severity::Error,
        summary: "wildcard import",
        explanation: "\
E0104 is reserved for a wildcard `from <module> import *` statement, which \
pulls an unbounded, not-statically-enumerable set of names into scope -- \
incompatible with pycc's requirement that every binding be resolvable at \
compile time. It is not currently emitted: a wildcard import is rejected \
today by the same versioned `C0001` \"not implemented yet\" capability code \
every other unsupported import shape uses, not this by-design-rejection \
code (D-137) -- E0104 is reserved for when wildcard imports gain a \
dedicated, deliberate rejection path distinct from \"we haven't implemented \
this import shape yet\".",
        example: "\
from math import *
",
    },
    DiagnosticExplanation {
        code: "E0105",
        severity: Severity::Error,
        summary: "metaclass with non-static side effects",
        explanation: "\
E0105 is reserved for a class using a custom `metaclass=` whose \
construction has runtime side effects pycc cannot evaluate ahead of time \
(as opposed to a purely structural metaclass pycc's compiler could resolve \
statically). It is not currently emitted: this is part of the planned v0.7 \
dynamic-Python rejection surface, not yet implemented; pycc has no class or \
metaclass support yet, so this check has nothing to guard today.",
        example: "\
class Meta(type):
    def __new__(mcs, name, bases, ns):
        print(\"class created\")  # a runtime side effect at class-creation time
        return super().__new__(mcs, name, bases, ns)

class C(metaclass=Meta):
    pass
",
    },
    DiagnosticExplanation {
        code: "E0106",
        severity: Severity::Warning,
        summary: "`__del__` relies on refcount timing",
        explanation: "\
E0106 is documented as `warning→error` in `docs/DIAGNOSTICS.md`: today's \
severity is `warning` (its current, initial documented state), and \
escalating it to a hard `error` is documented future intent, not yet \
implemented -- the `Severity` enum currently has only `Error`/`Warning`, no \
third \"will become an error\" state. It is reserved for a `__del__` method \
whose behavior depends on exactly when CPython's refcounting frees the \
object -- pycc's own refcounting model (with RC-elision optimizations, \
MEMORY_OWNERSHIP.md) does not guarantee identical collection timing to \
CPython, so `__del__` logic relying on precise timing is unreliable under \
pycc. It is not currently emitted by any compiler pass: pycc has no class \
support yet, so there is no `__del__` method for this check to inspect.",
        example: "\
class Resource:
    def __del__(self) -> None:
        print(\"freed now\")  # timing may differ from CPython's refcounting
",
    },
    DiagnosticExplanation {
        code: "E0107",
        severity: Severity::Error,
        summary: "`sys.getrefcount` unavailable",
        explanation: "\
E0107 is reserved for a call to `sys.getrefcount`, which exposes CPython's \
own internal reference count -- a value pycc's compiled output has no \
equivalent for once RC-elision optimizations remove some of the increments \
and decrements CPython would have performed (`id()` remains stable across \
moves and is supported; `getrefcount` is not). It is not currently \
emitted: this is part of the planned v0.7 dynamic-Python rejection surface, \
not yet implemented; `sys` is not even a registered stdlib module in \
pycc's frontend yet (D-136), so this call falls through to `C0001` today.",
        example: "\
import sys

def f(x: object) -> int:
    return sys.getrefcount(x)
",
    },
    DiagnosticExplanation {
        code: "E0108",
        severity: Severity::Error,
        summary: "import cycle in top-level init",
        explanation: "\
E0108 is reserved for a cycle in module-level (top-level) initialization \
order reached through `import` statements -- two or more modules whose \
top-level code each depends on the other having already run. It is not \
currently emitted: pycc compiles exactly one module per invocation today \
(true multi-file support is planned for v0.4, D-094), so there is no \
cross-module import graph yet for a cycle to exist in.",
        example: "\
# a.py
import b

# b.py
import a  # cycle: a's top level needs b, b's top level needs a
",
    },
    DiagnosticExplanation {
        code: "I0401",
        severity: Severity::Error,
        summary: "untyped value leaks across interop boundary",
        explanation: "\
I0401 is reserved for a value crossing the planned CPython interop \
boundary (v0.7) without a type pycc's static checker can verify on the \
other side -- e.g. a value returned from an embedded-CPython call whose \
runtime type is not pinned down by any annotation. It is not currently \
emitted: the interop boundary itself (`--interop-policy`, D-128) is planned \
v0.7 work and does not exist in this pycc version yet.",
        example: "\
# I0401 is reserved for pycc's not-yet-implemented CPython interop
# boundary (v0.7); no current pycc source can reach this check.
",
    },
    DiagnosticExplanation {
        code: "I0402",
        severity: Severity::Error,
        summary: "CPython-backed direct import root rejected by the effective interop policy",
        explanation: "\
I0402 is reserved for an `import` whose resolved root package is only \
available as a CPython-backed (not pycc-native) module, and is rejected by \
the effective v0.7 `[interop]` policy (`auto`/`allowlist`/`deny`, D-128) in \
force for the project. It is not currently emitted: the `[interop]` table \
and both interop CLI flags (`--interop-policy`, `--pure`) are themselves \
planned v0.7 features and are not implemented or enforced by the current \
compiler.",
        example: "\
# I0402 is reserved for pycc's not-yet-implemented CPython interop
# policy enforcement (v0.7); no current pycc source can reach this check.
",
    },
    DiagnosticExplanation {
        code: "W1001",
        severity: Severity::Warning,
        summary: "unreachable code",
        explanation: "\
W1001 is reserved for a statement pycc's compiler can prove will never \
execute -- code after an unconditional `return`/`raise` in the same block, \
for example. It is not currently emitted by any compiler pass: pycc has no \
reachability/control-flow analysis wired up to detect and report this yet. \
(It does appear as convenient sample data in `pycc_diag`'s own unit tests \
for the generic `render_human`/`render_json` renderers, but those tests \
exercise the renderer's \"a warning with no span\" code path in the \
abstract -- they are not evidence of a real unreachable-code detection \
pass.)",
        example: "\
def f() -> int:
    return 1
    print(\"this line can never run\")
",
    },
    DiagnosticExplanation {
        code: "W1002",
        severity: Severity::Warning,
        summary: "boxed fallback in hot loop (`--memstats` hint)",
        explanation: "\
W1002 is reserved for `pycc build --memstats`'s planned ownership/\
allocation report (MEMORY_OWNERSHIP.md): a hint that a value inside a hot \
loop fell back to a boxed (heap-allocated, refcounted) representation \
where an unboxed one might have been possible. It is not currently \
emitted: `--memstats` itself is planned v0.4 work (escape analysis, move \
semantics, RC elision) and is not implemented in this pycc version yet.",
        example: "\
# W1002 is reserved for pycc's planned --memstats ownership report (v0.4);
# no current pycc source can reach this check.
",
    },
];

/// Looks up a diagnostic code's long-form explanation by exact,
/// case-sensitive string match -- matching every existing diagnostic
/// code's own always-uppercase convention, so no case-insensitive
/// normalization is performed.
pub fn find(code: &str) -> Option<&'static DiagnosticExplanation> {
    EXPLANATIONS.iter().find(|entry| entry.code == code)
}

fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

/// `pycc explain`'s human-readable output: a header line of the shape
/// `code (severity): summary`, a blank line, the long-form explanation
/// paragraph, a blank line, and an indented `Example:` block. See
/// `docs/CLI_SPEC.md`'s "`pycc explain` output contract" section for the
/// full contract this reproduces.
pub fn render_explanation_human(entry: &DiagnosticExplanation) -> String {
    // Every `EXPLANATIONS` example is a short, non-blank-line snippet (see
    // `completeness_test_covers_every_registered_code_and_severity`'s
    // sibling tests for the full table), so this deliberately does not
    // special-case an empty line to skip its indentation -- that branch
    // would have no real content to exercise it under D-014's coverage
    // gate, and a blank line would just gain trailing spaces instead,
    // which is harmless for `Example:` block output.
    let mut indented_example = String::new();
    for line in entry.example.lines() {
        indented_example.push_str("    ");
        indented_example.push_str(line);
        indented_example.push('\n');
    }
    format!(
        "{} ({}): {}\n\n{}\n\nExample:\n{}",
        entry.code,
        severity_word(entry.severity),
        entry.summary,
        entry.explanation,
        indented_example
    )
}

/// `pycc explain <code> --format json`'s output: `format_version: 1`,
/// `kind: "diagnostic_explanation"` (the disambiguator against `pycc
/// check`'s own, differently-shaped `format_version: 1` diagnostic-array
/// JSON -- see `docs/CLI_SPEC.md`), plus `code`/`severity`/`summary`/
/// `explanation`/`example`.
pub fn render_explanation_json(entry: &DiagnosticExplanation) -> String {
    let value = serde_json::json!({
        "format_version": 1,
        "kind": "diagnostic_explanation",
        "code": entry.code,
        "severity": severity_word(entry.severity),
        "summary": entry.summary,
        "explanation": entry.explanation,
        "example": entry.example,
    });
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn find_returns_some_for_a_known_code() {
        let entry = find("T0001").expect("T0001 is registered");
        assert_eq!(entry.code, "T0001");
        assert_eq!(entry.severity, Severity::Error);
    }

    #[test]
    fn find_returns_none_for_an_unknown_code() {
        assert!(find("X9999").is_none());
    }

    #[test]
    fn render_explanation_human_matches_the_documented_shape() {
        let entry = find("T0001").expect("T0001 is registered");
        let rendered = render_explanation_human(entry);
        assert!(rendered.starts_with("T0001 (error): public function missing annotation\n\n"));
        assert!(rendered.contains("\n\nExample:\n    def add(a: int, b: int) -> int:\n"));
        assert!(rendered.ends_with("        return a + b\n"));
    }

    #[test]
    fn render_explanation_json_matches_the_documented_schema() {
        let entry = find("E0106").expect("E0106 is registered");
        let rendered = render_explanation_json(entry);
        let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed["format_version"], 1);
        assert_eq!(parsed["kind"], "diagnostic_explanation");
        assert_eq!(parsed["code"], "E0106");
        assert_eq!(parsed["severity"], "warning");
        assert_eq!(parsed["summary"], "`__del__` relies on refcount timing");
        assert!(
            parsed["explanation"]
                .as_str()
                .unwrap()
                .contains("warning→error")
        );
        assert!(parsed["example"].as_str().unwrap().contains("__del__"));
    }

    /// Drift guard (D-150): parses `docs/DIAGNOSTICS.md`'s "Initial
    /// registry" table at test time (never at runtime -- see this module's
    /// own doc comment) and asserts, both directions, that `EXPLANATIONS`'s
    /// code set exactly matches the documented registry's code set, and
    /// that every shared code's severity matches too. `.unwrap()`/
    /// `.expect()` are used freely here on this repository's own
    /// known-good file (matching `tests/slice0.rs`'s existing precedent of
    /// `.unwrap()`-ing `serde_json::from_str` inside a test body) rather
    /// than factored into a fallible helper -- there is deliberately no
    /// error-return code path here for D-014's coverage gate to require a
    /// fixture for.
    #[test]
    fn completeness_test_covers_every_registered_code_and_severity() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let diagnostics_md =
            std::fs::read_to_string(format!("{manifest_dir}/../../docs/DIAGNOSTICS.md"))
                .expect("docs/DIAGNOSTICS.md must be readable from pycc_diag's own manifest dir");

        // Registry table rows look like:
        // | `T0001` | error | public function missing annotation |
        // Only scan the "## Initial registry" section: the "## Code
        // ranges" table above it also has a backtick-quoted first cell
        // (e.g. `| `C0xxx` | ... |`), so a plain "starts with a
        // backtick-quoted cell" check alone would wrongly pick up its
        // range rows too. Scope by section heading instead, then extract
        // the code cell and the severity cell for every row in it.
        let registry_section = diagnostics_md
            .split("## Initial registry")
            .nth(1)
            .expect("docs/DIAGNOSTICS.md must have an \"## Initial registry\" section")
            .split("\n## ")
            .next()
            .expect("split always yields at least one piece");

        let mut documented: HashSet<(String, String)> = HashSet::new();
        for line in registry_section.lines() {
            let line = line.trim();
            if !line.starts_with("| `") {
                continue;
            }
            let mut cells = line.split('|').map(str::trim).filter(|c| !c.is_empty());
            let code_cell = cells.next().expect("a registry row has a code cell");
            let severity_cell = cells.next().expect("a registry row has a severity cell");
            let code = code_cell.trim_matches('`').to_string();
            // Normalize "warning→error" (E0106's escalation-intent
            // annotation) to its leading word before "→", generically --
            // any future escalating-severity code needs no special case
            // here, only this same normalization.
            let severity = severity_cell
                .split('→')
                .next()
                .expect("split always yields at least one piece")
                .trim()
                .to_string();
            documented.insert((code, severity));
        }

        let documented_codes: HashSet<&str> =
            documented.iter().map(|(code, _)| code.as_str()).collect();
        let explained_codes: HashSet<&str> = EXPLANATIONS.iter().map(|entry| entry.code).collect();
        assert_eq!(
            documented_codes, explained_codes,
            "EXPLANATIONS's code set must exactly match docs/DIAGNOSTICS.md's registry table"
        );

        for entry in EXPLANATIONS {
            let expected_severity = documented
                .iter()
                .find(|(code, _)| code == entry.code)
                .map(|(_, severity)| severity.as_str())
                .expect("every EXPLANATIONS code was just confirmed to be documented");
            assert_eq!(
                severity_word(entry.severity),
                expected_severity,
                "{}'s EXPLANATIONS severity must match docs/DIAGNOSTICS.md (after \
                 warning→error normalization)",
                entry.code
            );
        }
    }
}
