# pycc CLI Specification

gcc-familiar, cargo-ergonomic. Same commands, flags, and output on Linux/macOS/Windows.

## Commands

| Command | Does |
|---|---|
| `pycc build [PATH] -o OUT` | compile to native binary (default `--debug`) |
| `pycc run [PATH] [-- args]` | build + execute |
| `pycc check [PATH]` | frontend only: parse + types + ownership; ruff-fast, no codegen |
| `pycc test` | run project tests compiled (pytest-style discovery, subset) |
| `pycc explain CODE` | long-form doc for a diagnostic (`pycc explain T0021`) |
| `pycc init [NAME]` | scaffold `pycc.toml` + `src/main.py` |
| `pycc clean` | drop `.pycc/` cache |
| `pycc version --verbose` | compiler, LLVM, target list |

`PATH` = file or project dir (uses `pycc.toml`); omitted → current dir.

## Key flags

```
--release           optimizations on (LTO, RC elision aggressive), asserts kept per config
--target TRIPLE     cross-compile (bundled lld): x86_64-pc-windows-msvc from a Mac just works
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

`0` ok · `1` compile errors · `2` bad invocation · `101` compiled program panicked/uncaught exception (matches process exit conventions per-OS).

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
