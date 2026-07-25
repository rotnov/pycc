# pycc CLI Specification

gcc-familiar, cargo-ergonomic. Same commands, flags, and output on Linux/macOS/Windows.

## Commands

| Command | Does |
|---|---|
| `pycc build [PATH] -o OUT` | compile to native binary (default `--debug`) |
| `pycc run [PATH] [-- args]` | build + execute |
| `pycc check PATH...` | frontend only: parse + HIR + types for every explicit file; no codegen |
| `pycc test` | run project tests compiled (pytest-style discovery, subset) |
| `pycc explain CODE` | long-form doc for a diagnostic (`pycc explain T0021`) |
| `pycc init [NAME]` | scaffold `pycc.toml` + `src/main.py` |
| `pycc clean` | drop `.pycc/` cache |
| `pycc version --verbose` | compiler, LLVM, target list |

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
--release           optimizations on (LTO, RC elision aggressive), asserts kept per config
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

`0` ok · `1` compile errors · `2` bad invocation or unreadable input · `101`
compiled program panicked/uncaught exception (matches process exit conventions
per-OS).

`pycc check` reports all supplied-file failures. If different files produce
both compile errors and unreadable-input errors, it exits `2`; otherwise any
compile error exits `1`.

## Diagnostics output contract

Human format (stable enough to screenshot, not to parse):

```
error[T0021]: argument 1 of `fib` expects `int`, got `str`
 --> src/main.py:5:15
  |
5 |     print(fib("35"))
  |               ^^^^ expected `int`
  = help: did you mean `int("35")`?
```

JSON format versioned (`"format_version": 1`), one object per diagnostic: code, severity, spans[{file,line,col,len,label}], message, help[], fix{edits[]}?. Consumed by editors and the corpus bot (TESTING.md).

Displayed diagnostic paths are lexically normalized without filesystem
canonicalization: native path separators are rendered as `/`, redundant `.`
components and repeated separators are removed, and `..` components are
preserved. A literal backslash in a Unix filename remains a backslash.
Control characters and Unicode bidirectional-formatting controls in displayed
paths and source excerpts are rendered as visible escapes so filenames or
source text cannot inject terminal controls or visually reorder diagnostics.
Literal source tabs remain tabs, and ordinary Unicode joiners remain intact.
Human-format caret padding measures each complete non-tab Unicode sequence
rather than summing individual scalar widths, so combining marks, emoji
modifiers, and zero-width joiner sequences align with the rendered source.
