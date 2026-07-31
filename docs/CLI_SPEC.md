# pycc CLI Specification

gcc-familiar, cargo-ergonomic. Same commands, flags, and output on Linux/macOS/Windows.

## Commands

| Command | Does |
|---|---|
| `pycc build [PATH] -o OUT` | compile to native binary; debug by default, unless `--release` or a neighboring `pycc.toml`'s `opt = "release"` says otherwise (see `--release` below) |
| `pycc run [PATH] [-- args]` | build + execute |
| `pycc check PATH...` | frontend only: parse + HIR + types for every explicit file; no codegen |
| `pycc test` | run project tests compiled (pytest-style discovery, subset) |
| `pycc explain CODE` | long-form doc for a diagnostic (`pycc explain T0021`) |
| `pycc init [NAME]` | scaffold `pycc.toml` + `src/main.py`; refuses to overwrite an existing `pycc.toml`, non-directory `src`, or `src/main.py` (exit 2, nothing written) |
| `pycc clean` | drop `.pycc/` cache |
| `pycc version --verbose` | compiler, LLVM, target list |

`pycc init` inspects every scaffold destination before writing anything: an
existing `pycc.toml`, a `src` that is not a directory, or an existing
`src/main.py` is a refusal (exit 2) that leaves all existing paths
byte-for-byte unchanged, and the scaffold writes `pycc.toml` last so a late
failure in the `src` steps can never leave it behind (#237's regressions are
pinned by `tests/slice0.rs`'s init suite and `src/project_config.rs`'s unit
injections). An existing `src/` directory is not itself a conflict — only its entry type and `main.py`'s presence are checked. Both file writes use create-new semantics, so a dangling symlink at either destination fails cleanly instead of writing through it, and a write that fails after creating its file removes that partial file again — a genuine I/O failure leaves no scaffold residue behind for a retry to trip over.

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
--error-format human|json     json = stable schema for editors/CI
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
allow = ["numpy", "requests"]   # modules permitted through the CPython escape hatch; empty = pure

[test]
paths = ["tests/"]
```

## Exit codes

`0` ok · `1` compile errors (including `C0001` version-capability gaps) · `2`
bad invocation or unreadable input · `101` compiled program panicked/uncaught
exception, or `build`/`run` hit one of `pycc_codegen`'s own explicit,
named "not supported yet" boundaries for a construct `pycc check` accepts but
codegen doesn't yet implement (D-072; the older D-035 `pycc_mir` boundary
this row used to name is closed for good as of PR-5 -- see D-072's own
Context) (matches process exit conventions per-OS). Unsupported HIR input
to `check` is a normal exit-1 diagnostic, not exit 101.

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
