# pycc CLI Specification

gcc-familiar, cargo-ergonomic. Same commands, flags, and output on Linux/macOS/Windows.

## Commands

| Command | Does |
|---|---|
| `pycc build [PATH] -o OUT` | compile to a deployment artifact; debug by default, unless `--release` or a neighboring `pycc.toml`'s `opt = "release"` says otherwise (see `--release` below) |
| `pycc run [PATH] [-- args]` | build + execute |
| `pycc check PATH...` | frontend only: parse + HIR + types for every explicit file; no codegen |
| `pycc test` | run project tests compiled (pytest-style discovery, subset) |
| `pycc explain CODE` | long-form doc for a diagnostic (`pycc explain T0021`) |
| `pycc init [NAME]` | scaffold `pycc.toml` + `src/main.py`; refuses to overwrite an existing `pycc.toml`, non-directory `src`, or `src/main.py` (exit 2, nothing written) |
| `pycc clean` | drop `.pycc/` cache |
| `pycc version --verbose` | compiler, LLVM, target list |

The current compiler and every future native or `deny`/`--pure` build write a
native binary at `OUT`. Planned v0.7 builds with a permitted CPython-backed
import instead use `OUT` as the deployment-artifact destination for an
autonomous application bundle. D-128 deliberately defers the bundle's exact
file layout until the v0.7 resolver and packaging plan is accepted.

`pycc init` inspects every scaffold destination before writing anything: an
existing `pycc.toml`, a `src` that is not a directory, or an existing
`src/main.py` is a refusal (exit 2) that leaves all existing paths
byte-for-byte unchanged, and the scaffold writes `pycc.toml` last so a late
failure in the `src` steps can never leave it behind (#237's regressions are
pinned by `tests/slice0.rs`'s init suite and `src/project_config.rs`'s unit
injections). An existing `src/` directory is not itself a conflict — only its entry type and `main.py`'s presence are checked. Both file writes use create-new semantics, so a dangling symlink at either destination fails cleanly instead of writing through it, and a write that fails after creating its file removes that partial file again — if that removal itself fails too, the cleanup failure is folded into the returned error rather than discarded, so a caller is never told only "the write failed" while a partial file silently remains on disk. If `pycc.toml`'s own write fails after `main.py` was already created by the same invocation, that `main.py` is rolled back too (and the `src/` directory it created, only when left empty), so a retry after fixing the underlying cause is not blocked by scaffold residue (#256) — pre-existing content is never touched, only entries this invocation itself created. A rollback that cannot remove a file it created reports that failure in the same error rather than silently claiming no residue remains. An unavailable current directory (deleted, unmounted, or otherwise inaccessible after launch) is an invocation/environment error reported as exit 2 with a stable `error: pycc init failed: cannot read current directory: <OS error>` diagnostic, not a panic (#251); no scaffold write is attempted and no fallback directory is used.

`pycc version` prints one summary line; `--verbose` appends the Tier-1 target
list, in the exact set and order of ARCHITECTURE.md's "Cross-platform (hard
requirement)" table. The compiler and rustc fields come from the crate
manifest; the LLVM field states D-015's pinned contract version.
`tests/slice0.rs`'s two version snapshot tests enforce the output shape —
manifest-sourced fields, the LLVM pin, and the exact target set and order;
the version numbers in the transcript below are illustrative and track the
manifest at build time:

```text
$ pycc version --verbose
pycc 0.1.0 (rustc 1.97.1, LLVM 22.1.1)
tier-1 targets:
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
  x86_64-apple-darwin
  aarch64-apple-darwin
  x86_64-pc-windows-msvc
```

The current v0.1 slice requires at least one explicit file for `pycc check` and
accepts multiple files in one invocation, matching the argument shape used by
pre-commit. It checks every supplied file before exiting. Directory discovery,
an omitted path meaning the current project, and `pycc.toml` project loading
arrive with multi-file projects in v0.4. The ownership pass joins `check` when
`pycc_own` is introduced in v0.5.

Paths are parsed in the operating system's native representation, so Unix
filenames with non-UTF-8 bytes reach file access losslessly; diagnostics use a
lossy display form only when text must be printed. Use `pycc check -- PATH...`
when a filename may begin with `-`. The published hook includes that boundary,
so normal `-h` and `--help` handling remains available.

Before any frontend command parses a file, pycc decodes its bytes using
Python's source-encoding rules: UTF-8 by default, an optional UTF-8 BOM, and an
encoding declaration on the first or eligible second line. The v0.1 decoder
supports UTF-8, strict ASCII, and true ISO-8859-1/Latin-1. Other declared
encodings are unreadable-input errors until pycc has a decoder with mappings
that exactly match Python's codec. A BOM/cookie conflict or malformed encoded
input is also an unreadable-input error. Runs of `-` and `_` in declared codec
labels are collapsed before alias resolution, matching Python's ASCII codec
normalization for the cookie grammar. Alias lookup first tries the normalized
label with dots intact, then retries with dots treated as separators, matching
Python's codec registry behavior for aliases such as `us.ascii` and
`iso.8859.1`. BOM agreement uses the tokenizer's stricter normalization: case
is folded, `_` becomes `-`, and repeated separators are not collapsed. After
decoding, LF, CRLF, and CR physical line endings are normalized to LF before
parsing and diagnostic span calculation.

For the other commands, the target contract remains `PATH` = file or project
directory (using `pycc.toml`), with an omitted path meaning the current
directory once project mode exists.

## Key flags

```
--release           LLVM's O3-equivalent whole-module optimization pipeline (D-094); no explicit flag or neighboring pycc.toml `opt = "release"` default builds debug (unoptimized). True cross-file LTO awaits v0.4's multi-file support (one module per compilation today); aggressive RC elision (v0.5's `pycc_own`) and per-config asserts are not implemented yet.
--target TRIPLE     cross-compile: currently proven same-OS/cross-arch only (e.g. macOS x64⟷arm64, D-026); cross-OS targets not yet supported
--emit mir|llvm-ir|obj|asm
--int hybrid|native|bigint    int repr override (default hybrid, D-001) — native = documented CPython deviation
--lib               emit C-ABI library + header instead of executable
--memstats          ownership/allocation report (see MEMORY_OWNERSHIP.md)
--interop-policy auto|allowlist|deny
                    planned v0.7 policy for CPython-backed imports (D-128);
                    CLI value overrides `[interop].policy`
--pure              planned v0.7 shorthand for `--interop-policy deny`;
                    conflicts with an explicit `--interop-policy`
--error-format human|json     json = stable schema for editors/CI (check only)
--format human|json           json = stable schema for editors/CI (explain only; deliberately not --error-format -- explain's output is never an error, see D-150)
--fix               apply machine-applicable suggestions (check only)
-j N                parallelism (default: cores)
```

## `pycc.toml`

```toml
[project]
name = "myapp"
entry = "src/main.py"
python = "3.14"          # language level; only 3.14 in v1

[build]
opt = "release"          # default profile for `pycc build`
targets = ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"]
static = true

[interop]
policy = "allowlist"      # planned v0.7: "auto" (default), "allowlist", or "deny"
allow = ["numpy", "requests"]   # direct import roots; used only by "allowlist"

[test]
paths = ["tests/"]
```

The `[interop]` table and both interop CLI flags are a **planned v0.7
contract**, not current compiler behavior. The current v0.1 TOML parser accepts
and ignores unmodeled future sections, and the current frontend rejects every
`import` before policy evaluation. When v0.7 implements this schema:

- omitting `[interop]` selects `policy = "auto"`, so a standard source import
  such as `import numpy as np` automatically resolves, pins, and bundles the
  compatible CPython runtime and package closure recorded in `pycc.lock`;
- `policy = "allowlist"` permits only the direct CPython-backed import roots
  named by `allow`; importing a submodule of an allowed root and loading its
  locked transitive closure do not require separate entries, while another
  direct root fails with `I0402`;
- `policy = "deny"`, `--interop-policy deny`, and `--pure` reject every
  CPython-backed dependency and guarantee that the produced artifact contains
  no CPython/libpython runtime; and
- the selected policy never changes native pycc-module imports. `allow` must
  be absent or empty outside `allowlist`, so a stale list cannot look
  authoritative while another policy silently ignores it.

The same effective policy applies to `check`, `build`, `run`, and `test`; the
eventual `pycc test` compilation path cannot bypass the project's dependency
policy. A CLI `--interop-policy` overrides the project setting; `--pure` is
rejected as an invalid invocation when combined with any explicit
`--interop-policy` rather than relying on argument order.

## Exit codes

`0` ok (including `pycc explain` on a recognized code, in either
`--format`) · `1` compile errors (including `C0001` version-capability gaps)
· `2` bad invocation, unreadable input, a toolchain/environment failure such
as a host linker driver that cannot be started (reported as an actionable
`error:` diagnostic, never a panic), or an unrecognized `pycc explain` code
(always a plain stderr message, regardless of `--format` -- see below)
· `101` compiled program panicked/uncaught
exception, or `build`/`run` hit one of `pycc_codegen`'s own explicit,
named "not supported yet" boundaries for a construct `pycc check` accepts but
codegen doesn't yet implement (D-072; the older D-035 `pycc_mir` boundary
this row used to name is closed for good as of PR-5 -- see D-072's own
Context) (matches process exit conventions per-OS). Unsupported HIR input
to `check` is a normal exit-1 diagnostic, not exit 101.

Named D-072 boundaries include, each recognizable by its exact message.
This list is maintained by hand and is **not** guaranteed exhaustive -- a
`pycc_codegen: ... is not supported yet` panic that names a construct is a
probable alpha boundary even when it is absent here, and should be checked
against `crates/pycc_codegen/src/lib.rs` before being treated as a defect:

- `pycc_codegen: using print()'s result as a nested expression is not
  supported yet` -- `print(...)` types as `None`, and an already-materialized
  `None` value does flow through `alloca`/parameter/return slots since D-075
  and D-131; what remains unlowered is `print()`'s **call result
  specifically**, which codegen never materializes at all (the panic is
  unconditional on the callee being `print`).
- ``pycc_codegen: string repetition (`str * int`) is not supported yet`` --
  `str * int` and `int * str` (with a `bool` count) type-check to `str` as of
  #574, but codegen has no repetition primitive yet; native repetition lands
  with #575.
- ``pycc_codegen: string conversion of a class instance without `__repr__` is
  not supported yet`` -- `print(p)` type-checks for any argument, and
  `pycc_mir`'s `rewrite_instance_to_repr` is a no-op for a class that defines
  no `__repr__`, so the instance reaches codegen's `to_str` unconverted
  (#378).
- The container `to_str` and truthiness boundaries D-107 records, reached the
  same way (`print(xs)` and `if xs:` type-check for any argument type, but
  v0.2 lowers neither conversion nor `bool(...)` for containers):
  `pycc_codegen: string conversion of a list[T] value is not supported yet`,
  and the same message for `dict[K, V]`, `set[T]`, and `tuple[...]`; plus
  `pycc_codegen: truthiness of a list[T] value is not supported yet`, and the
  same message for `dict[K, V]`, `set[T]`, and `tuple[...]`.

Each is an intentional alpha boundary, not a reportable compiler defect.

`pycc run` normalizes every unsuccessful generated-program termination to
`101`. This includes an ordinary non-zero child status, a Unix signal (which
has no numeric `ExitStatus::code()`), and a platform abort status wider than
the CLI's portable one-byte exit-code range; raw child status values are not
part of the CLI contract.

`pycc check` reports all supplied-file failures. If different files produce
both compile errors and unreadable-input errors, it exits `2`; otherwise any
compile error exits `1`.

## Diagnostics output contract

Human format (stable enough to screenshot, not to parse):

```
error[T0021]: argument 1 of `fib` expects `int`, got `str`
 --> src/main.py:1:1
  |
1 | def fib(n: int) -> int:
  | ^ argument 1 of `fib` expects `int`, got `str`
```

Every `T0xxx` diagnostic's span is currently the `Span::new(0, 0)` placeholder
(`line 1, column 1`, one-character caret) regardless of where the real error
is, and the caret label always repeats the diagnostic's full message rather
than an independent short label -- both are current, real behavior, not an
aspirational target (D-043).

JSON format versioned (`"format_version": 1`), one object per diagnostic: code, severity, spans[{file,line,col,len,label}], message, help[], fix{edits[]}?. `line` and `col` are 1-indexed Unicode-scalar positions; `len` counts Unicode scalar values from the span start, including normalized line separators in a multi-line span. Consumed by editors and the corpus bot (TESTING.md).

`help[]` holds exactly one entry for a diagnostic whose message already states a determinate, safe fix (an exact expected type, an exact expected count, an exact "add an annotation" instruction, an already-embedded usage example, or a self-contained constraint the message itself already names, such as a literal-index requirement), and is empty otherwise (D-152). This is currently true for arity/type-mismatch, missing-annotation, and literal-index-constraint diagnostic families; name-resolution, capability-limitation, and ambiguous-conflict diagnostics still emit `help: []`. The human format above has no `help:` line codepath at all, regardless of whether `help[]` is populated in JSON (D-043, D-083).

Displayed diagnostic paths are lexically normalized without filesystem
canonicalization: native path separators are rendered as `/`, redundant `.`
components and repeated separators are removed, and `..` components are
preserved. A literal backslash in a Unix filename remains a backslash.
Control characters and Unicode bidirectional-formatting controls in displayed
paths and source excerpts are rendered as visible escapes so filenames or
source text cannot inject terminal controls or visually reorder diagnostics.
Literal source tabs remain tabs, and ordinary Unicode joiners remain intact.
Human-format caret padding handles the current wide-character and common
combining-mark blocks, emoji modifiers, and well-formed zero-width-joiner
emoji sequences as terminal sequences rather than summing every scalar as one
column. Full Unicode terminal-width conformance remains future work.

## `pycc explain` output contract

`pycc explain CODE [--format human|json]` prints long-form documentation for
a diagnostic code registered in [DIAGNOSTICS.md](./DIAGNOSTICS.md)'s
"Initial registry" table (D-150, `crates/pycc_diag/src/explain.rs`). This is
a **different, unrelated JSON shape** from the "Diagnostics output contract"
section above: that section documents an *occurred* diagnostic (a real
compile error/warning against a specific file and span); this section
documents a *code in the abstract*, with no file or span involved at all.
Both happen to start with `"format_version": 1`, which is why `explain`'s
JSON output carries an additional `"kind": "diagnostic_explanation"` field
the diagnostic-occurrence schema does not have (and must not gain here --
that schema's codes and JSON structure are intentionally stable, per its own
quality bar in DIAGNOSTICS.md) -- a consumer holding a bare JSON blob with no
side channel can always tell the two apart by checking for that field.

Human format:

```
T0001 (error): public function missing annotation

<long-form explanation of the real trigger condition>

Example:
    def add(a: int, b: int) -> int:
        return a + b
```

JSON format (`--format json`), one object, distinct from `check`'s own
diagnostic-occurrence JSON:

```json
{
  "format_version": 1,
  "kind": "diagnostic_explanation",
  "code": "T0001",
  "severity": "error",
  "summary": "public function missing annotation",
  "explanation": "<long-form explanation of the real trigger condition>",
  "example": "def add(a: int, b: int) -> int:\n    return a + b\n"
}
```

An unrecognized code exits `2` with a plain stderr message
(`error: unknown diagnostic code \`CODE\``) in either `--format` -- an
unrecognized code is an out-of-band invocation failure, not a diagnostic
occurrence, so it is never itself subject to `--format`, matching how
`check`'s own out-of-band `FrontendFailure::Input` class ("could not
read ...") is never subject to `--error-format` either. Code lookup is an
exact, case-sensitive string match; every registered code is always
uppercase, so no case normalization is performed.
