# pycc CLI Specification

gcc-familiar, cargo-ergonomic. Same commands, flags, and output on Linux/macOS/Windows.

## Commands

| Command | Does |
|---|---|
| `pycc build [PATH] -o OUT` | compile to a deployment artifact; `PATH` and every project module it imports (see "Project imports" below) are linked into one program; debug by default, unless `--release` or a neighboring `pycc.toml`'s `opt = "release"` says otherwise (see `--release` below) |
| `pycc run [PATH] [-- args]` | build + execute; every imported module's top-level statements run before the entry file's, in dependency order |
| `pycc check PATH...` | frontend only: parse + HIR + link + types for every explicit file *and its import closure*; reports every diagnostic the failing pass found, each rendered against the file that owns it (parser fan-out since #864 Part 1, D-217; HIR lowering per top-level item with cascade suppression since Part 2, D-219; the type checker one per failing item, solver-first, since Part 3, D-220; per-file attribution across a linked program since #898, D-222); no codegen |
| `pycc lock PATH [--check]` | record the CPython dependency closure `PATH`'s embedded build will carry into `pycc.lock`, read offline from the `PYCC_PYTHON` interpreter's installed environment; `--check` writes nothing and exits 1 when the lock is not current (see "`pycc.lock`" below) |
| `pycc test` | run project tests compiled (pytest-style discovery, subset) |
| `pycc explain CODE` | long-form doc for a diagnostic (`pycc explain T0021`) |
| `pycc init [NAME]` | scaffold `pycc.toml` + `src/main.py`; refuses to overwrite an existing `pycc.toml`, non-directory `src`, or `src/main.py` (exit 2, nothing written) |
| `pycc clean` | drop `.pycc/` cache |
| `pycc version --verbose` | compiler, LLVM, target list |

A program with no CPython-backed import writes a native binary at `OUT`, and
so does every successful `deny`/`--pure` build, since those policies reject every
CPython-backed import with `I0402` (#1224). A program with a CPython-backed
import builds an **embedded executable** on a macOS or Linux host without
`--target` (Part 1 of #1028, D-128's `auto` default), or on a Windows host
as a stub `OUT` plus `OUT.pycc\` (#1286, D-253; a pure-Python locked closure
since #1296; see `PYCC_PYTHON` below): the executable at `OUT`
plus an `OUT.pycc/` sidecar directory holding the embed interpreter's shared
library and filtered standard library and, when the program imports a root
outside the standard library, the dependency closure `pycc.lock` records,
copied into `OUT.pycc/closure/` (#1242; see "`pycc.lock`" below). With
`--static-libpython` or `[build] static = true`, the executable links
libpython statically instead and the sidecar holds no copy of it (D-251; see
`--static-libpython` below). The two move
together and must keep their names; D-248 owns the layout, the `PYCC-BUNDLE`
marker, and the rule that an existing `OUT.pycc` without that marker is never
replaced (exit 2). An embedded build refuses an `OUT` file name containing `$`
or `:` (exit 2). The effective interop policy (see `pycc.toml` below) is
decided first, per import: a root it rejects is `I0402`; an admitted import
the build cannot embed (an excluded Tcl/Tk root or a `--target` build) is
`I0403`; and an admitted root outside the standard library
without a current `pycc.lock` section is exit 2 naming `pycc lock`. The
hosted `ext` mode is the exception to both (D-244 rule 1):
`pycc build PATH -o OUT --ext` writes a CPython extension module at `OUT` and
never an executable or a bundle. The recognized extension suffixes are the
target platform's own `importlib.machinery.EXTENSION_SUFFIXES`, which CPython
orders most-specific first: on Linux and macOS the version-and-platform-tagged
suffix, then `.abi3.so`, then `.so`; on Windows the tagged suffix, then `.pyd`.
When `OUT` names none of them, `.abi3.so` is appended on Linux and macOS and
`.pyd` on Windows -- the stable-ABI spelling, since rule 1 builds against
`Py_LIMITED_API`. A recognized suffix already on `OUT` is honored as
written when it is version-agnostic -- `.abi3.so` or `.so` on Linux and macOS,
`.pyd` on Windows, each of which every host's finder searches -- while the
interpreter-specific tagged suffix (`m.cpython-314-x86_64-linux-gnu.so`,
`m.cp314-win_amd64.pyd`) is **rejected** rather than honored: rule 1 builds one stable-ABI artifact per platform that
every later GIL-enabled host is meant to load, and a version-tagged filename
hides it from exactly those hosts, whose finders search their own tag, then
`.abi3.so`, then `.so`, and never an earlier interpreter's tag. The module
name is `OUT`'s basename with the **first matching** suffix from that ordered
list removed, which is exactly how CPython's own finder derives a name from
the same file: `m.abi3.so` yields `m`, never `m.abi3`. Stripping only the
final `.so` would derive a name the finder never uses and emit a `PyInit_`
symbol no host would look for. The derived name must be a valid **ASCII**
Python identifier, and it is the `<mod>` in the exported `PyInit_<mod>`, so an
artifact is importable only under the name its own output path spells. A
non-ASCII identifier is rejected rather than encoded: CPython loads such a
module through `PyInitU_<punycode>` with hyphens replaced by underscores --
`mód` through `PyInitU_md_5ja`, verified against a live interpreter -- so
emitting `PyInit_mód` would produce an artifact no host can import.
A basename that reduces to `__init__` is a valid ASCII identifier, so
that rule admits it, and it is **rejected** separately: CPython's finder
discovers `pkg/__init__.abi3.so` as the module `pkg` -- the name comes
from the parent directory, and the spec it builds carries that directory
as `submodule_search_locations`, verified against a live interpreter --
and then looks for `PyInit_pkg`, a name this contract derives from the
file rather than from the directory holding it. The rejection is a scope
boundary, not a limitation of the host: rule 1 specifies one extension
module per artifact, and a package `__init__` is a packaging shape `ext`
mode does not claim. Deriving `<mod>` from the parent directory is the
wider rule and stays available later, since widening a rejection
supersedes nothing.
D-128's `--interop-policy` and `--pure` do not apply in this mode (D-244
rule 3) and are rejected alongside it, as is `--lib`.

Every value after `pycc run`'s `--` is forwarded unchanged and in order as
the generated program's own process arguments, including a value that
itself starts with `-` (e.g. `-x`, `--flag`) -- once past `--`, nothing is
interpreted as a `pycc` option (#23). Each forwarded value is a native
process argument, not text: `pycc` parses these trailing forwarded
arguments as `OsString` (matching the `PATH`/`OUT` filesystem-path
arguments on `build`/`run`/`check`, which are native `PathBuf`s and
preserve non-UTF-8 bytes the same way, #249), so a non-UTF-8 value after
`--` is forwarded as the same opaque byte sequence instead of being
rejected with a CLI parse error (#824). Omitting `-- args` entirely runs
the program with no arguments, same as before this contract existed.

`run` has flags of its own since #1224 (`--interop-policy`, `--pure`), and it
recognizes them in a window: before `PATH`, and between `PATH` and the first
forwarded value. `pycc run app.py --pure` sets `--pure`. The first value that
is not a recognized `pycc` flag, and every value after it, is forwarded:
`pycc run app.py x --pure` forwards `x` and `--pure`, and an unrecognized
hyphen value such as a misspelled `--pur` is forwarded too, taking every
later value with it. `--` ends the window explicitly, so
`pycc run app.py -- --pure` forwards `--pure`; it is the always-safe form for
a program argument that could be mistaken for a `pycc` flag. The conflict
between `--pure` and an explicit `--interop-policy` holds among the flags
`pycc` parses, in any order within the window; a value past the window is a
program argument and conflicts with nothing. `pycc run app.py extra` and
`pycc run app.py -- extra` still parse identically.

`pycc init` inspects every scaffold destination before writing anything: an
existing `pycc.toml`, a `src` that is not a directory, or an existing
`src/main.py` is a refusal (exit 2) that leaves all existing paths
byte-for-byte unchanged, and the scaffold writes `pycc.toml` last so a late
failure in the `src` steps can never leave it behind (#237's regressions are
pinned by `tests/slice0.rs`'s init suite and `src/project_config.rs`'s unit
injections). An existing `src/` directory is not itself a conflict — only its entry type and `main.py`'s presence are checked. Both file writes use create-new semantics, so a dangling symlink at either destination fails cleanly instead of writing through it, and a write that fails after creating its file removes that partial file again — if that removal itself fails too, the cleanup failure is folded into the returned error rather than discarded, so a caller is never told only "the write failed" while a partial file silently remains on disk. If `pycc.toml`'s own write fails after `main.py` was already created by the same invocation, that `main.py` is rolled back too (and the `src/` directory it created, only when left empty), so a retry after fixing the underlying cause is not blocked by scaffold residue (#256) — pre-existing content is never touched, only entries this invocation itself created. A rollback that cannot remove a file it created reports that failure in the same error rather than silently claiming no residue remains. An unavailable current directory (deleted, unmounted, or otherwise inaccessible after launch) is an invocation/environment error reported as exit 2 with a stable `error: pycc init failed: cannot read current directory: <OS error>` diagnostic, not a panic (#251); no scaffold write is attempted and no fallback directory is used.

`pycc version` prints one summary line; `--verbose` appends the Tier-1 target
list, in the exact set and order of ARCHITECTURE.md's "Cross-platform (hard
requirement)" table. The `pycc` field comes from the crate manifest
(`CARGO_PKG_VERSION`); the `rustc` field is the actual compiler that built
this binary, captured by the root `build.rs` at build time rather than read
from the manifest's `rust-version` MSRV contract, which can diverge from it
(#247); the LLVM field states D-015's pinned contract version.
`tests/slice0.rs`'s two version snapshot tests enforce the output shape —
the manifest-sourced `pycc` field, the build-time `rustc` field (checked
against `OUT_DIR/rustc_version.txt`, a second build-time artifact `build.rs`
writes alongside the `rustc`-env var it injects into `src/main.rs`, read
through `env!("OUT_DIR")` rather than re-invoking `rustc` at test run time,
since Cargo only sets `RUSTC` for the build script itself), the LLVM pin,
and the exact target set and order; the version numbers in the transcript
below are illustrative and track the manifest/toolchain at build time:

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
pre-commit. It checks every supplied file before exiting. Within a file, every
diagnostic the first failing pass collected is reported, in that pass's own
order; the first diagnostic for any input was kept byte-identical (code,
message, and span) across the three #864 parts as a transition invariant
(D-217 rule 2), not as a release-to-release promise: D-233 lets an enum-call
`C0001` scanned from an earlier item precede a later item's own diagnostic.
HIR lowering collects, per
top-level item, the item's own diagnostic when it fails (skipping that item)
plus one enum-call `C0001` per call of an enum class inside it that the
scan can attribute (D-233; a call whose name is shadowed, rebound, or
otherwise outside the scan's enumerated limits falls through to the type
checker's span-less guard at `1:1` instead), so one item can contribute
several diagnostics; an item whose only
failure is a reference to a class or type alias that itself failed to lower
contributes no lowering diagnostic of its own rather than a second gap (D-219);
the enum-call scan still runs on such an item, so a call to another, valid
enum class inside it is still reported. The type
checker reports one diagnostic per failing function (D-220). A pre-check
failure (an incompatible redefinition or attribute redeclaration) is
reported alone. Otherwise, if the private-helper solver's list is
module-level (a failure in its top-level walk or in a post-body phase such
as `propagate_binop_constraints`), that one diagnostic is reported alone
and the annotation checker's list is dropped, because a post-body solver
diagnostic cannot be matched by function to the checker's entry for the
same error. Otherwise the solver's per-function diagnostics are reported in
item order, then every checker entry -- per-function or module-level --
whose function the solver did not flag, in the checker's order. If the
solver passes, the checker's list against the solved signatures is reported
on its own.

### Project imports

Since #898 (D-222) `check`, `build`, and `run` resolve
`from <project module> import <name>` across files. The dotted module name
resolves against a *source root*: the directory named by a `pycc.toml`'s
`[project].entry` (searched for in the entry file's directory and its
ancestors), or -- with no `pycc.toml` -- the first directory above the entry
file's own package chain (the walk stops at the first directory with no
`__init__.py`). A stdlib module name always wins over a same-named project
file. Relative imports (`from .mod import x`) resolve against the importer's
package. The two mechanisms are ordered, not exclusive: when a `pycc.toml`
is found but the directory named by its `[project].entry` cannot be
canonicalized, or is not an ancestor of the checked file, discovery falls
back to the package walk rather than reporting a diagnostic. Root discovery
is lazy: a file with no project import never pays for it.

A dependency's diagnostics render against the dependency's own path, and an
unreadable or undecodable dependency is exit 2 exactly like an unreadable
input named on the command line. `pycc check a.py b.py` still processes each
path independently, so when `b.py` imports `a.py`, `a.py`'s diagnostics print
once under each path -- a cross-invocation incremental cache is a later v0.4
item. Type diagnostics are attributed per item, so a diagnostic from a
dependency's function or top-level statement names that file; a pre-check or
module-level solver failure names the entry file. All `pycc_types` spans
remain `0,0` until #877.

Directory discovery, an omitted path meaning the current project, and
`pycc.toml` project *mode* (its `[project].entry` as the default build
target) remain deferred. The ownership pass joins `check` when
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
--release           LLVM's O3-equivalent whole-module optimization pipeline (D-094); no explicit flag or neighboring pycc.toml `opt = "release"` default builds debug (unoptimized). True cross-file LTO awaits v0.4's separate compilation (one linked program per compilation today: the entry file and its whole import closure are linked into a single module before optimization, #898/D-222); aggressive RC elision (v0.5's `pycc_own`) and per-config asserts are not implemented yet.
--target TRIPLE     cross-compile: currently proven same-OS/cross-arch only (e.g. macOS x64⟷arm64, D-026); cross-OS targets not yet supported
--emit mir|llvm-ir|obj|asm
--int hybrid|native|bigint    int repr override (default hybrid, D-001) — native = documented CPython deviation
--lib               emit C-ABI library + header instead of executable
--ext               hosted mode: emit a CPython extension module instead of
                    an executable (D-244 rule 1; see the `OUT` contract
                    above). `docs/RUNTIME.md`'s `ext` boundary section is
                    the canonical admissibility matrix for which signatures
                    are implemented; any other public signature is rejected
                    as `C0003`; the
                    export set is the whole linked program -- the entry file
                    and its import closure (D-222) -- so a public function
                    in an imported project module is exported too, and it
                    reaches a public `@staticmethod`/`@classmethod` of a
                    public class, published as `mod.Class.method` (#1143),
                    and a public instance method of a public class that
                    some *published, constructible* class's method
                    resolution order reaches, called on an instance the
                    host builds with
                    `mod.Class(...)` (#1145) -- each class's method table is
                    resolved through the class's namespace, so an
                    inherited method is reachable on the derived class while
                    any binding the derived class makes for that name --
                    another method, a `@property`, an `@abstractmethod` --
                    shadows it, publishing the derived binding or nothing;
                    `docs/RUNTIME.md`'s `ext` boundary section states which
                    classes are published and which are constructible.
                    `--ext` imports a non-standard-library root from the
                    host's environment and needs no `pycc.lock`; an
                    embedded build bundles it from the lock (#1242).
                    Conflicts with `--interop-policy` and
                    `--pure` (exit 2, D-244 rule 3); will conflict with
                    `--lib` once that flag exists.
--static-libpython  embedded build only: link libpython into the executable
                    from the embed interpreter's static archive
                    (`sysconfig` `LIBPL/LIBRARY`, e.g.
                    `libpython3.14.a`) instead of bundling its shared
                    library, and export its C-API symbols so the bundled
                    `lib-dynload` modules resolve against the executable
                    (D-251, Part 1 of #1227). The archive must be a
                    regular `ar` archive -- a missing file, a thin archive,
                    or a file that is not an archive (such as a
                    `libpython3.14.a` symlink to the shared library) is
                    exit 2 -- and a bundled image that needs a shared
                    libpython, a locked closure's included, is refused
                    (exit 2). A `pycc.lock` section is consumed as in a
                    shared build (Part 2 of #1227, #1272): its
                    `libpython-sha256` is checked against the interpreter's
                    shared library, or against the archive when it is
                    configured without one. The sidecar keeps the standard library, and its marker
                    records the archive's digest and `libpython-link
                    static`. No explicit flag falls back to a neighboring
                    pycc.toml's `[build] static = true`. A build that
                    embeds no interpreter (native, `--pure`, `--target`)
                    ignores it; `--ext` rejects it (exit 2); `run` has no
                    such flag and always links the shared library. A
                    Windows host refuses it at exit 2 before probing the
                    interpreter: CPython for Windows ships no static
                    library (D-251, D-253).
                    Force-loading makes every archive member's own
                    dependencies mandatory: an archive whose built-in
                    modules need libraries outside `LIBS`/`SYSLIBS`, or a
                    non-PIC archive in a PIE link, fails in the linker
                    (exit 1).
--memstats          ownership/allocation report (see MEMORY_OWNERSHIP.md)
--interop-policy auto|allowlist|deny
                    embedded-mode policy for CPython-backed imports in
                    `build`, `run`, `check` and `lock` (D-128, #1224);
                    CLI value overrides `[interop].policy`
--pure              shorthand for `--interop-policy deny`; conflicts
                    with an explicit `--interop-policy` (exit 2)
--error-format human|json     json = stable schema for editors/CI (check only)
--format human|json           json = stable schema for editors/CI (explain only; deliberately not --error-format -- explain's output is never an error, see D-150)
--fix               planned: apply machine-applicable suggestions (check
                    only); not yet implemented -- currently rejected as an
                    unrecognized argument, not specially recognized
-j N                parallelism (default: cores)
```

## Environment

`pycc build` looks for `pycc_rt`'s static library in the *Cargo target
directory*, which Cargo — not `pycc` — produces. Building this workspace
puts it there: `pycc_codegen`'s build script builds `pycc_rt` for both
host profiles and installs the archives at the resolved location, so a
clean checkout needs no separate `cargo build -p pycc_rt` step (D-184).
Resolution precedence,
keeping Cargo's own relative order for the inputs `pycc` can observe
(D-183):

1. **`CARGO_TARGET_DIR`**, when set to a non-empty value.
2. otherwise **`CARGO_BUILD_TARGET_DIR`**, when set to a non-empty value.
   This is Cargo's generic config-to-environment mapping of the
   `build.target-dir` config key; Cargo honors it whether or not any
   `.cargo/config.toml` exists, and `CARGO_TARGET_DIR` outranks it.
3. otherwise **`<workspace root>/target`**.

An empty value at either level is treated as unset. That is a deliberate
divergence from Cargo, which rejects an empty `CARGO_TARGET_DIR` outright
(exit 101, "the target directory is set to an empty string ...") rather
than falling back: honoring it here would resolve artifacts to a bare
relative `debug/`, and an exported-but-empty variable is a shell accident
rather than an intent to redirect.

A relative value is used as-is, not re-anchored on the workspace root.
Cargo resolves a relative target directory against the working directory
of the process that invoked *it*; `pycc` is a separate process, so this
agrees with Cargo when `pycc` runs from that same directory.

Two inputs rank above all three levels in Cargo's own precedence and are
**permanently not honored**: the `--target-dir` **command-line flag**, and
`build.target-dir` when set in a `.cargo/config.toml` **config file**.
Reading the config-file form means re-implementing Cargo's
ancestor-walking config discovery — the `$CARGO_HOME` merge, Cargo's own
precedence and path-resolution rules — a materially larger surface than
this gap. The flag's resolved path does reach an integration-test or
bench binary, but only through the compile-time `CARGO_TARGET_TMPDIR`
macro and never the runtime environment, and not at all to the `pycc`
binary a user invokes; anchoring the shared resolver on it would give one
function two resolution rules depending on which binary it was compiled
into. A build whose
artifacts were redirected by one of those two, with neither environment
variable set, resolves to the `<workspace>/target` fallback on both the
write side and the read side — `pycc_codegen`'s build script installs the
`pycc_rt` archives there and the driver searches there — so it links
correctly, at the cost of a stray `<workspace>/target` tree beside the
directory Cargo actually built into. Whenever the archive is absent from
that resolved root (the build script has not run for the profile or
`--target` triple in use, or the artifact was cleaned), the driver fails
with the ordinary actionable exit-2 message naming the directory that was
searched (`no pycc_rt build found (expected ...). Run \`cargo build -p
pycc_rt\` first, or run pycc with CARGO_TARGET_DIR set to the directory
Cargo built into.`) rather than mislinking.
The environment-variable precedence above is the complete contract; both
exclusions are a closed decision, not a pending gap
([D-216](./decisions/D-216-close-the-target-dir-flag-and-config-file.md)).

Within the resolved directory the layout is Cargo's: `<root>/debug/` or
`<root>/release/` for a host build, `<root>/<triple>/<profile>/` when
`--target` is given.

The `no pycc_rt build found ... Run \`cargo build -p pycc_rt\` first, or
run pycc with CARGO_TARGET_DIR set to the directory Cargo built into.`
diagnostic names both recoveries on its single line, because the
situations that still produce it split two ways: a cross-compilation
target the build script does not produce (`--target <triple>`, which
additionally needs `rustup target add <triple>`) and a one-off
`--target-dir` build are fixed by running the suggested `cargo build`
command by hand, while a target directory persistently redirected by an
input `pycc` cannot observe — when the archive is also absent from the
`<workspace>/target` fallback the build script installs into — is fixed by
pointing `pycc` at that directory through `CARGO_TARGET_DIR`. The second
clause was added by [#869](https://github.com/rotnov/pycc/issues/869)
because the `CARGO_TARGET_DIR` recovery below was otherwise undiscoverable
from the terminal; it follows the same convention as the diverged-root
warning in `crates/pycc_codegen/build.rs`, which already tells the user to
set an absolute `CARGO_TARGET_DIR`. The diagnostic is no longer an
ordinary first-build message. Under a config-file
`build.target-dir` redirect (or a `--target-dir` flag the user repeats on
the recovery command, say through a shell alias) the suggested command is
not sufficient on its own, because `cargo build -p pycc_rt` honors the
same redirect that `pycc` cannot see and writes the archive there; a bare
`cargo build -p pycc_rt` after a one-off `--target-dir` build lands in
`<workspace>/target` and does work. For the persistent case, either set
`CARGO_TARGET_DIR` to the redirected directory so `pycc` follows it, or
run `CARGO_TARGET_DIR=<workspace>/target cargo build -p pycc_rt` so the
rebuild lands where `pycc` searches — `CARGO_TARGET_DIR` outranks a
config-file `build.target-dir` (verified under both cargo 1.88.0 and the
pinned 1.97.1: with `.cargo/config.toml` naming `from-config`, a plain
build wrote there and `CARGO_TARGET_DIR=from-env` redirected to
`from-env`).

Two further variables locate CPython for `build --ext`, and the first also
names the interpreter an embedded build bundles (see the end of this
section):

- **`PYCC_PYTHON`** names the interpreter to probe for its `include`
  directory, its `libs` directory and its version. Default: `python3`.
  The interpreter is run once, with a fixed `-c` script and no shell; it
  must satisfy the stable-ABI floor (CPython 3.13, D-244), and an
  interpreter that cannot be started, exits non-zero, or prints something
  unparseable is an environment failure at exit 2.
- **`PYCC_PYTHON_INCLUDE`**, when set, supplies that header directory
  directly and **no interpreter is run at all** — for a cross build, or a
  sysroot whose interpreter cannot execute on the building host. The
  `libs` directory is then taken as its sibling. Because nothing runs,
  nothing can read the headers' real version: setting this variable is an
  assertion that they are at least the stable-ABI floor, and headers that
  are not fail later in the C compiler rather than in `pycc`. It outranks
  `PYCC_PYTHON`, which is still recorded and still named in diagnostics.

Whichever variable supplies it, the resolved header directory is checked
before anything is compiled: it must exist and it must contain `Python.h`.
A directory that is missing, or that exists but holds no header, is an
environment failure at exit 2 — an interpreter installed without its
development package is a broken build environment, not a defect in the
source being compiled, and reporting it as one would misclassify it as a
compile error.

For an **embedded** build (a plain `build` or `run` whose CPython imports
are all standard-library roots), `PYCC_PYTHON` names the interpreter to embed
and defaults to `python3.14`, because D-128 pins CPython 3.14.
`PYCC_PYTHON_INCLUDE` has no effect there. The interpreter must be a shared,
non-free-threaded CPython 3.14.x with its headers, shared library and
standard library present (a `--static-libpython` build instead needs its
static archive at `LIBPL` and accepts an interpreter built without a shared
library, D-251), and every native library its libpython and
`lib-dynload` modules link must be a system library or lie under its own
prefix (on Linux, too, since #1243); each failure is an environment failure
at exit 2 naming the reason (D-248 rules 4 and 5). A build with no CPython import runs no interpreter,
and neither variable affects it.

On a **Windows host** (#1286, D-253) `PYCC_PYTHON` defaults to
`python3.14.exe`. The interpreter needs no shared-library check (CPython for
Windows reports none), but its `libs\python314.lib` and `libs\python3.lib`
import libraries, its `python314.dll` and `python3.dll`, and its `DLLs\`
directory must exist, each missing one an exit-2 failure naming the path. The
build links a program DLL (`OUT.pycc\pycc_program.dll`) against
`python314.dll`, and a stub `OUT` that imports only the system DLLs `KERNEL32`
and `ntdll` and loads that
DLL from its own sidecar; the sidecar holds `python314.dll`, `python3.dll`,
the interpreter's `vcruntime140.dll` and `vcruntime140_1.dll` when present,
and the filtered `Lib\` and `DLLs\`, plus `closure\` when the program's
`pycc.lock` section records one (#1296, D-249). `--static-libpython` and
`[build] static = true` are refused at exit 2 (D-251), and so is a locked
closure holding a file Windows would load as a PE image (a `.pyd` or `.dll`
suffix, or an `MZ` header on any suffix other than `.exe`), naming #1297 and
`pycc build --ext`.

`pycc lock` reads `PYCC_PYTHON` the same way and refuses the interpreters an
embedded build refuses, except that it accepts one configured without a shared
library as a `--static-libpython` build does, since one lock serves either kind of
build (#1272); it then reads that interpreter's own
`sysconfig` `purelib` and `platlib` directories, typically a project venv's
(see "`pycc.lock`" below). `PYCC_PYTHON_INCLUDE` has no effect on it.

## `pycc.toml`

```toml
[project]
name = "myapp"
entry = "src/main.py"
python = "3.14"          # language level; only 3.14 in v1

[build]
opt = "release"          # default profile for `pycc build`
targets = ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"]
static = false           # `true` links libpython statically into an embedded executable; needs the interpreter's `LIBPL/libpython3.14.a` (D-251)

[interop]
policy = "allowlist"      # "auto" (default), "allowlist", or "deny"
allow = ["numpy", "requests"]   # direct import roots; used only by "allowlist"

[test]
paths = ["tests/"]
```

`[build] static` means "statically link every library the artifact would
otherwise load dynamically, where pycc supports it" (D-251). Today that is
exactly libpython in an embedded `pycc build`, the same as
`--static-libpython`; a native artifact, `--ext` and `pycc run` ignore it, and
an absent key keeps each artifact's own default. `true` makes every embedded
build need the interpreter's static archive, so a copied manifest that sets it
turns a missing archive into an exit-2 refusal that names both the key and the
flag. On a Windows host, `static = true` is refused at exit 2 before the probe,
like `--static-libpython` (D-251, D-253).

The `[interop]` table and both interop CLI flags are implemented (#1224),
and everything in this section describes the **embedded** mode only: an
`--ext` build ignores the table entirely, never validating it, and rejects
both flags (D-244 rule 3, mirrored from `RUNTIME.md`'s canonical statement).
The TOML parser still accepts and ignores other unmodeled sections such as
`[test]`. What each policy admits is current behavior; what an admitted root
then builds is bounded by the embedding (D-248): a standard-library root
builds an embedded executable with no lock, and any other admitted root
builds one bundling its closure from `pycc.lock` (#1242). A root imported
only inside a `try` whose handler catches a failed import is optional
(#1290, the **Source** rule below): it still needs the lock, but when it is absent the lock records no
package for it and the program's handler runs.

- omitting `[interop]` selects `policy = "auto"`, which admits every
  CPython-backed root. The target contract is that a standard source import
  such as `import numpy as np` then resolves, pins, and bundles the
  compatible CPython runtime and package closure recorded in `pycc.lock`;
  `pycc lock` records the closure (D-249) and an embedded build bundles it
  (#1242), with the native libraries it needs outside the interpreter
  (#1243);
- `policy = "allowlist"` permits only the direct CPython-backed import roots
  named by `allow`, and another direct root fails with `I0402`. A locked
  root's transitive closure loads without separate entries for its
  dependencies (#1242); importing a submodule of an allowed root is still
  `C0001` today. Each entry is one root name, so an empty or dotted entry is
  invalid;
- `policy = "deny"`, `--interop-policy deny`, and `--pure` reject every
  CPython-backed import with `I0402`, so the produced artifact contains no
  CPython/libpython runtime; and
- the selected policy never changes native pycc-module imports (`import math`
  stays native under `deny`) or project imports. `allow` must be absent or
  empty outside `allowlist`, so a stale list cannot look authoritative while
  another policy silently ignores it.

The effective policy is an explicit `--interop-policy` or `--pure`, else the
`[interop]` table of the program's manifest, else `auto`. The manifest is the
one `pycc.toml` the module loader discovers for the program, walking up from
the entry file's directory, so one program has exactly one. A CLI switch to
`allowlist` uses the configured `allow` list when the table itself selects
`allowlist`, and an empty list otherwise, rejecting every root. The policy is
resolved only when the program has a CPython-backed import: a program without
one, including one whose only imports are project modules, never validates
the table. When it is resolved, the table is always validated, even when a
flag overrides it: an unknown key, a `policy` other than `"auto"`,
`"allowlist"` or `"deny"`, a value of the wrong type, an empty or dotted
`allow` entry, or a non-empty `allow` outside `allowlist` is an input error
naming the manifest (exit 2), as is a manifest the loader cannot parse.

The policy is decided per import before the embedding: a root it rejects is
`I0402` on every host and under `--target`, never `I0403`, and a root it
admits proceeds to D-248's embedding, where it may still be `I0403`. The
`I0402` message names the policy and where it was set: the flag, or the
manifest's path as the command spelled it. `check` has no artifact mode, so
it checks this embedded contract: a project that configures `deny` but ships
only `--ext` artifacts gets `I0402` from `check` while `build --ext` succeeds.

The same effective policy applies to `check`, `build`, `run`, and `test`; the
eventual `pycc test` compilation path cannot bypass the project's dependency
policy. `pycc lock` applies it too: a root it rejects is `I0402`, exit 1, before anything is written. A CLI `--interop-policy` overrides the project setting; `--pure` is
rejected as an invalid invocation when combined with any explicit
`--interop-policy` rather than relying on argument order.

## `pycc.lock`

`pycc lock PATH` records the CPython dependency closure `PATH`'s embedded
build will carry. [D-249](./decisions/D-249-pycc-lock-schema-environment-resolver-and-update-command.md) owns the
contract; this section summarizes it. Part 1 of #1225 (#1241) implements the
file and the command, Part 2 (#1242) the build consuming it, Part 3 (#1243)
native libraries outside the interpreter, and Part 4 (#1259) macOS relative
references outside a distribution's payload.

- **Source.** The closure is read offline from the installed `*.dist-info`
  distributions in the `PYCC_PYTHON` interpreter's `sysconfig` `purelib` and
  `platlib` directories, never from user site-packages, `PYTHONPATH` or
  `.pth` files, and with no network and no version solving. The direct roots
  are the program's CPython-backed import roots the interop policy admits,
  minus the standard-library roots; the closure follows their owners'
  `Requires-Dist` with environment markers evaluated for that interpreter,
  and refuses anything it cannot evaluate. A direct root is *optional*
  (#1290) when every import of it sits in the body of a `try` (or
  `try`/`except*`) at module level or nested only in module-level
  `if`/`try` blocks, with a bare `except:` or a handler naming
  `ImportError`, `ModuleNotFoundError` or `Exception`, alone or in a tuple,
  at any depth; a handler, `else` or `finally` body is not guarded by its
  own `try`. One unguarded import makes the root required. Optional roots
  are recorded as `optional-roots` and required ones as `roots`.
- **Integrity.** Every file of a locked distribution is re-hashed against
  its RECORD; a mismatch, a missing or symlinked file, an editable install, a
  top-level `.pth` file, an unowned root or an on-disk file under a root that
  no RECORD lists refuses the lock (exit 2, naming the file). Each package
  records a `tree-sha256` over its payload. An *unowned optional* root is
  not refused: it is recorded with no package, and its on-disk files are
  still checked, so an unrecorded file, a symlink, or a directory holding
  no file (a namespace package CPython would import) under it refuses the
  lock. An optional root reachable only through a `.pth` file or an
  editable finder is not detected (D-249 rule 1).
- **Location and key.** The file is `pycc.lock` beside the nearest
  `pycc.toml` above `PATH`, else beside `PATH`. One file holds one section
  per (entry, host triple), where `entry` is the canonical entry path
  relative to the lock's directory, so every spelling of one script is one
  key. The file is TOML, `version = 1`, sorted, with no absolute path or
  timestamp; a reader refuses another version, an unknown field, a duplicate
  section, a non-Tier-1 triple or an `entry` with an empty, `.` or `..`
  component (so an absolute one too).
- **Update.** `pycc lock PATH` replaces the (entry, host) section, drops
  sections whose entry script no longer exists, and writes the file through
  a temporary `pycc.lock.tmp-<pid>` and a rename. Concurrent runs against
  one file are not serialized: the last rename wins, and `--check` reports
  a section it dropped. A program with no
  CPython-backed import has no section and never starts the interpreter; a
  lock left with no sections is deleted. A standard-library-only program
  gets the interpreter fields and `roots = []`, without a site scan.
  `optional-roots` is written only when the program has an optional root,
  so a lock without one is unchanged; an older pycc refuses the field.
- **Native libraries.** Each `[[target.native]]` entry is a library a
  closure image needs, directly or through another such library, that lies
  outside the system library directories and the interpreter's prefix and
  is not libpython: its name in `OUT.pycc/lib/` (the file name on macOS,
  the `DT_NEEDED` name on Linux), the sha256 of its bytes, and the sorted
  distributions that need it. macOS follows absolute install names; Linux
  resolves each `DT_NEEDED` as `ld.so` would on the build host
  (`DT_RPATH`/`DT_RUNPATH` with `$ORIGIN`, the `ldconfig -p` cache, then the
  default directories). A dependency it cannot find is left to the loader
  when an image loaded on import needs it, and refused when a library the
  build copies into `lib/` needs it.
- **`--check`.** Exits 0 only when the file's bytes equal what `pycc lock`
  would write, where a standard-library-only program with no section counts
  as current; otherwise it exits 1 naming the first difference and writes
  nothing.
- **Failures.** An unparsable existing lock, or one with another `version`,
  is exit 2 for both forms and is never overwritten. A Windows host writes a
  section like any other host (#1296): its ownership suffixes add `.pyw`,
  `.pyd` and `.<tag>.pyd`; a `RECORD` path inside the site that holds a `\`
  or a `:`, a component ending in `.` or a space, or a reserved device name
  (`CON`, `NUL`, `COM1`, ...) is refused; and the section's natives are
  always empty, since the build refuses a closure PE image until #1297.
- **Build.** An embedded build of a program with a root outside the standard
  library reads its (entry, host triple) section before probing the
  interpreter: a missing lock or section, or different `roots` or
  `optional-roots` (each compared on its own, and the refusal names the
  field), is exit 2 naming `pycc lock`, as is a malformed lock in any embedded build. After the probe, `python`, `cache-tag`,
  `platform` and `libpython-sha256` must equal the embed interpreter's (the
  digest is of its shared library, or of its `LIBPL` archive when it is
  configured without one -- by `Py_ENABLE_SHARED` and `PYTHONFRAMEWORK`,
  never by which files exist -- so a shared and a `--static-libpython` build
  check one lock alike, #1272), and
  each package's version, file set and every copied file's digest must match
  the lock and the installed RECORD (exit 2 otherwise, leaving an existing
  `OUT.pycc` untouched). The payload is copied to `OUT.pycc/closure/`, which
  the launcher appends to `sys.path`; a standard-library-only program needs
  no lock, gets no `closure/`, and has an existing section's interpreter
  fields checked. The build re-derives the natives and refuses a difference
  from `[[target.native]]` naming `pycc lock`, before writing anything; it
  copies each into `OUT.pycc/lib/`, refusing one whose bytes no longer
  match. On macOS the references to it are rewritten to the copy; on Linux
  the executable links every library copied into `lib/` by name, so the
  loader finds it already loaded. Refused with exit 2: two libraries needing
  one name in `lib/` (compared case-folded), and on Linux a library whose
  `DT_SONAME` differs from the name it is needed by, a `DT_NEEDED` given as
  a path, a closure program image that needs a copied library, a copied
  library that would also answer a dependency kept on the system, a copied
  library with a dependency that resolves nowhere, and a dependency on the
  interpreter's libpython under a name other than the bundled one. On macOS
  every dependency of a closure image or a native, absolute or
  `@rpath`/`@loader_path`, is resolved on the build host from the image's
  source directory and its own `LC_RPATH` entries: a locked payload file (of
  any locked distribution) is kept when the spelling already reaches it in
  the sidecar, and otherwise rewritten to an explicit `@loader_path` path to
  its `closure/` copy; a relatively named system library is rewritten to
  its absolute path; a file under a scanned site directory or
  `<stdlib>/site-packages` is a native even inside the prefix; anything
  else outside the prefix is a native. Refused with exit 2, naming the
  image and the reason: an `@executable_path` reference or an
  `@executable_path` rpath the search reaches before a match; a reference
  that resolves nowhere from the image's own rpaths; and any other form
  (neither absolute nor `@rpath`, `@loader_path` or `@executable_path`,
  such as a bare relative name). `pycc check` never reads the lock.

## Exit codes

`0` ok (including `pycc explain` on a recognized code, in either
`--format`) · `1` compile errors (including `C0001` version-capability gaps), or
`pycc lock --check` finding `pycc.lock` not current
· `2` bad invocation, unreadable input, a toolchain/environment failure such
as a host linker driver that cannot be started or an unusable system temp
directory in which `build`/`run` cannot create their scratch directory —
checked before any frontend work, so a bad temp directory fails fast
(reported as an actionable
`error:` diagnostic, never a panic), `pycc run`'s just-linked binary failing
to spawn (permission denied, or the file vanishing between link and spawn —
an ordinary environment failure reported as `error: could not run the built
program \`<path>\`: <OS error>`, never a panic), or an unrecognized `pycc
explain` code
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
`101`. An embedded program's exit status is user-controlled (`sys.exit(3)`
exits `3` when the built executable runs directly), but `pycc run` still maps
it to `101`, unchanged by #1223. This includes an ordinary non-zero child status, a Unix signal (which
has no numeric `ExitStatus::code()`), and a platform abort status wider than
the CLI's portable one-byte exit-code range; raw child status values are not
part of the CLI contract.

`pycc check` reports all supplied-file failures. Exit `1` means at least one
compile diagnostic in at least one file, however many diagnostics each file
produced. If different files produce both compile errors and unreadable-input
errors, it exits `2`; otherwise any compile error exits `1`.

`pycc check` writes diagnostics to stdout in the selected `--error-format`;
`pycc build` and `pycc run` write the same human renders to stderr (they have
no `--error-format`), every collected diagnostic, then stop before MIR.

Before `build` and `run` create their per-invocation scratch directory in
the system temp directory, they also opportunistically remove
provably-stale pycc-owned scratch roots left there by dead pycc processes
— silently, bounded, and best-effort: the sweep never changes a command's
output, diagnostics, or exit code (#784; see `pycc_scratch`'s `sweep`
module documentation for the exact staleness conditions and budgets).

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

Several diagnostics for one file are rendered as concatenated human renders with no separator, exactly as multi-file output already is.

JSON format versioned (`"format_version": 1`), one object per diagnostic, one object per line in report order (JSON Lines) -- a single file may therefore emit several lines, and `format_version` stays `1` because the per-object schema is unchanged (D-217). Each object carries: code, severity, spans[{file,line,col,len,label}], message, help[]; a planned `fix{edits[]}` field for machine-applicable suggestions is not yet emitted by the serializer (`crates/pycc_diag/src/lib.rs`'s `render_json`/`Diagnostic` have no `fix` key today) and awaits the same `--fix` implementation described above. `line` and `col` are 1-indexed Unicode-scalar positions; `len` counts Unicode scalar values from the span start, including normalized line separators in a multi-line span. Consumed by editors and the corpus bot (TESTING.md).

`help[]` holds exactly one entry for a diagnostic whose message already states a determinate, safe fix (an exact expected type, an exact expected count, an exact "add an annotation" instruction, an already-embedded usage example, or a self-contained constraint the message itself already names, such as a literal-index requirement), and is empty otherwise (D-152). This is currently true for arity/type-mismatch, missing-annotation, and literal-index-constraint diagnostic families; name-resolution, capability-limitation, and ambiguous-conflict diagnostics still emit `help: []`. The human format above has no `help:` line codepath at all, regardless of whether `help[]` is populated in JSON (D-043, D-083).

Report order is pass order (parser, then HIR, then types -- only one pass
fails per file, since each pass stops the pipeline), then that pass's own
collection order; the parser's is ruff's discovery order, which is not always
source order, HIR lowering's is the source order of the top-level items
(D-219), each item's own diagnostic first and then that item's enum-call
`C0001`s (D-233), and the type checker's is the solver's item-order walk of
function bodies followed by the annotation checker's item-order entries for
functions the solver did not flag, so a checker-only function may follow a
later solver-flagged one (D-220). No span-monotone order is promised across a
file's diagnostics (D-217); every `pycc_types` diagnostic still renders at
`:1:1` because the type pass does not carry spans yet (D-043).

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
