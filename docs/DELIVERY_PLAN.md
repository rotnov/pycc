# pycc Delivery Plan

How the roadmap in [ROADMAP.md](./ROADMAP.md) actually gets built: milestone decomposition, environment baseline, v0.1 execution strategy, and the autonomy policy that lets agents proceed without stopping for questions. Companion to [SPEC.md](./SPEC.md); written under D-013 (AI-first development).

## Program-level decomposition

Every ROADMAP.md milestone is its own sub-project: its own brainstorm → plan → implementation cycle, entered only when the previous milestone's acceptance criteria are green. Only v0.1 is detailed below (see [Delivery mechanics](#delivery-mechanics) for why); v0.2+ get the same treatment when reached, since their design will be shaped by what v0.1–v0.4 teach us.

| Milestone | New crates / major additions | Depends on | Rough PR count |
|---|---|---|---|
| v0.1 hello, binary | `pycc`, `pycc_lexer`/`pycc_parser`/`pycc_ast` (vendored `ruff_python_parser`, D-003), `pycc_hir`, `pycc_types`, `pycc_mir`, `pycc_codegen`, `pycc_rt`, `pycc_diag`, `pycc_testkit`, CI matrix on 5 Tier-1 targets | — | ~7 (detailed below) |
| v0.2 collections & generics | `pycc_std` created (`math`, `sys`), monomorphization in `pycc_types`/`pycc_mir`, `--release`/LTO profile, `pycc.toml` | v0.1 | ~5-6 |
| v0.3 classes & pattern matching | class model, dataclasses, protocols, `match` w/ exhaustiveness, diagnostics registry grows | v0.2 | ~6 |
| v0.4 projects & incremental | multi-file/import resolution in `pycc_hir`, salsa-style incremental cache, `os`/`pathlib`/`json`/`datetime` | v0.3 | ~6-7 |
| v0.5 generators & ownership v1 | new crate `pycc_own` (escape analysis, move semantics, RC elision), generators as state machines | v0.4 | ~6-7 |
| v0.6 threads without GIL | Shareable/move checks in `pycc_own`, cycle collector, own parser replaces vendored `ruff_python_parser` (D-003 resolved) | v0.5 | ~6 |
| v0.7 interop escape hatch | `pycc.interop.cpython` typed boundary | v0.6 | ~4-5 |
| v0.8 corpus at scale + bot | `corpus-bot` GitHub Action, `socket`/`http.client`/compression stdlib | v0.7 | ~4 |
| v0.9 async & packaging | `asyncio` subset on state machines, `--lib` C-ABI | v0.8 | ~4-5 |
| v1.0 spec freeze | PYTHON_STANDARDS Python 3.0–3.14 matrix complete, semantics deviations doc, benchmarks published, diagnostics/JSON frozen semver | v0.9 | ~3-4 |

## Environment baseline (verified, not assumed)

Verified empirically on the primary dev host (macOS, aarch64-apple-darwin) before committing to any of the below:

| Requirement | Finding | Consequence |
|---|---|---|
| Rust | `rustc 1.97.1` (stable, updated via `rustup update stable`; matches README's "1.97+" exactly) | Toolchain pinned via `rust-toolchain.toml` inside the repo — **not** a global `rustup default` change, since this machine has other toolchains (incl. a `solana` one) that must stay untouched |
| LLVM | `22.1.1` (single Homebrew keg; the `llvm@17`..`llvm@22` opt-paths are stale symlinks to the same keg, not distinct installs) | `inkwell = "0.9"` with feature `llvm22-1` — clean match, no version fudging needed |
| CPython oracle | `python3.14` → `3.14.3` at `/opt/homebrew/bin/python3.14` | Matches the v1 language line but is behind the current 3.14.6 patch target; upgrade before PR-6. |
| Local linker | Apple clang 21 / Xcode CLT `ld64` | Sufficient for the first vertical slice on native host; PR-3's `--target` work (D-026/D-028/D-031) routes through each host's own driver -- system `cc` (Apple clang) on macOS, bundled clang on Windows/Linux when `--target` is given -- not a universally bundled `lld` binary, which none of the three LLVM distributions this project installs actually ships |
| crates.io | Reachable (a bare `curl -I` 403s on crates.io's anti-bot filter — mundane, not a sandbox restriction; a real UA gets 200) | `cargo build` can fetch `ruff_python_parser`, `inkwell`, `rayon`, `mimalloc` |
| `gh` CLI | Authenticated, `repo`+`workflow` scopes | Can open PRs and push `.github/workflows` |
| `cargo-llvm-cov` | **Not** part of the rustup `llvm-tools` component — it is a separately distributed binary (own crate/release) that *uses* `llvm-tools-preview`'s `llvm-cov`/`llvm-profdata` at runtime. An earlier version of this plan conflated the two (caught by repo audit, issue #13); a spec that just says "install llvm-tools" fails in CI with "no such command: llvm-cov" | PR-1's CI skeleton installs both, explicitly and pinned: `rustup component add llvm-tools-preview` **and** a pinned `cargo-llvm-cov` install — never a bare "latest," per D-014's own no-unreviewed-drift spirit. CI smoke-checks the direct binary, then invokes it with the explicit `llvm-cov` subcommand under a clean unprivileged `nobody` environment whose workspace and trusted executables are read-only. A repository alias, `PATH` mutation, build script, or procedural macro therefore cannot replace the gate executable. |

Release review on 2026-07-24 found upstream Python 3.14.6 while the verified
local oracle above remains 3.14.3. The table records the actual host state, not
the desired PR-6 pin. Before PR-6, install and pin 3.14.6 locally and in CI,
re-verify the executable path/version, and re-record all differential outputs.

Only macOS is locally verifiable. Linux x64/arm64 and Windows MSVC exist only via CI — so CI must be wired right after the first local slice works, not as a v0.1 finishing touch (see PR-3 below).

## v0.1 crate scope

Not all 11 crates in [ARCHITECTURE.md](./ARCHITECTURE.md) are needed on day one.

**Built now:** `pycc` (CLI/driver), `pycc_parser`/`pycc_ast` (thin wrapper over vendored `ruff_python_parser`), `pycc_hir`, `pycc_types` (strict public annotations plus monomorphic local/private-helper inference over the v0.1 primitive subset), `pycc_mir`, `pycc_codegen` (LLVM via `inkwell`), `pycc_rt` (minimal runtime), `pycc_diag`.

**Deliberately deferred:** `pycc_own` (ownership/escape analysis is a v0.5 item — semantics-preserving, perf-only, so v0.1 just uses RC/heap without ownership inference), `pycc_std` (real importable modules start v0.2; the Tier-0 builtins v0.1 needs — `print`, `range`, etc. — live as intrinsics directly in `pycc_rt`/`pycc_codegen`), `pycc_lexer` (D-017 — the vendored parser bundles lexing internally, nothing consumes a standalone token stream yet), `pycc_testkit` (D-018 — no PEP conformance matrix exists yet for a real harness to check against; `tests/slice0.rs` covers PR-2's two named fixtures ad hoc in the meantime).

## v0.1 execution strategy: thin vertical slice, then breadth

Three approaches were weighed: (A) thin end-to-end slice first, then grow features; (B) parallel crate teams integrated at the end; (C) fully lock every crate interface up front, then build sequentially.

**A was chosen.** B reproduces the "horizontal slicing" anti-pattern the repo's own `tdd` skill warns against — interfaces designed independently by separate workstreams diverge at integration time, and a compiler pipeline's crates have real, not incidental, sequential dependencies. C is excess upfront design for a project whose own TESTING.md philosophy is "prove it with a test," not "prove it on paper" — and the biggest unknown (does LLVM/inkwell/linking/CI actually work end to end on 5 platforms) is exactly the thing A de-risks first instead of discovering three weeks into frontend work.

1. **Slice 0**: `def main() -> None: print(42)` compiled through the entire pipeline to a running native binary on the local host. Proves crate wiring before anything else.
2. **CI matrix** on all 5 Tier-1 targets immediately after slice 0 — not at the end. From this point, CI is the source of truth for autonomous progress, not local macOS runs.
3. **TDD red-green, vertical slices** for feature growth: one conformance test per feature, written first, then the implementation — arithmetic → comparisons → `if`/`while`/`for`+`range` → functions/recursion → basic f-strings.
4. **Parallelism** only where genuinely independent (CI tooling, `pycc_diag` registry scaffolding, `docs/semantics.md`) via subagents; the pipeline core stays sequential because its dependencies are real.

### Rough PR breakdown

| PR | Content |
|---|---|
| 1 | Workspace scaffold (all crate stubs), `rust-toolchain.toml` (1.97.1), CLI skeleton, CI skeleton **with the coverage gate wired in from the first commit** (D-014): pinned `cargo-llvm-cov` installer (it is a separate binary, not part of `llvm-tools` — see the Environment baseline note below) + the `llvm-tools-preview` rustup component, `cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100`. New ADRs appended to DECISIONS.md: LLVM 22.1/inkwell 0.9 pin, vendored `ruff_python_parser` version |
| 2 | Slice 0: parser → HIR → types (passthrough) → MIR → LLVM codegen → link; "hello binary" runs locally. Covers **both** entry shapes named in ROADMAP.md's v0.1 scope: a `main()`-defining module, and a module with only top-level statements and no `main()` — module-level execution is a named conformance case here, not an incidental side effect of the `main()` path |
| 3 | CI matrix live on all 5 Tier-1 targets for slice 0, **plus one cross-compiled build**: `pycc build --target x86_64-apple-darwin` from the `macos-14` (arm64) runner, executed and verified on `macos-15-intel` (x64) — same-OS/cross-arch, per D-026, since true cross-OS needs a target sysroot this project doesn't bundle yet (verified empirically, not assumed). Cross-compilation is a v0.1 requirement from D-011/README/ARCHITECTURE/CLI_SPEC, not a nice-to-have — it does not get to hide behind "CI matrix live" meaning same-host-per-target builds only |
| 4 | Frontend depth: full v0.1 grammar, real T0001 + local inference, first diagnostic codes with snapshot tests. First per-PR frontend benchmark baseline recorded here (see Performance gate below) since this is the first PR with a non-trivial frontend to measure |
| 5 | Codegen depth: full v0.1 feature set (int/float/str/bool, arithmetic, control flow, recursion, f-strings); runtime fleshed out (overflow→bigint per D-001, small-string opt per D-007). `--debug` profile only — `--release`/LTO is a v0.2 item (see Testing scope below), not built here |
| 6 | `pycc_testkit`: fib + mandelbrot-ascii vs. pinned CPython 3.14.6 on all 5 targets, `--debug` profile; `pycc check` benchmark <50ms/1k LOC; diagnostic output matches CLI_SPEC.md's example byte-for-byte |
| 7 | Buffer: close whatever's left so all v0.1 ROADMAP.md acceptance bullets are simultaneously green |

Each row above absorbs one gap an automated repo audit found in the original version of this plan (tracked as GitHub issues #9–#13): the cross-compilation gap (#9, → PR-3), the debug/release conformance contradiction (#10, → PR-5/PR-6 and Testing scope below), the missing module-level-execution case (#11, → PR-2), the missing early performance gate (#12, → PR-4 and Performance gate below), and the `cargo-llvm-cov` installation error (#13, → PR-1 and Environment baseline below). Issue #14 (measure platform-specific code per-platform rather than exempting it) is deferred — there is no platform-specific code yet for the question to apply to; it rides along whenever D-014 next gets touched.

### Performance gate (resolves #12)

ARCHITECTURE.md requires benchmarks in CI on every PR with a >2% frontend-regression merge block. This was originally scheduled to start immediately once PR-4 made the frontend non-trivially executable, deliberately not deferred to a single check in PR-6 (D-042/D-044): every subsequent PR would record a `pycc check` timing run and fail if it regressed >2% against the latest baseline published by a successful `main` run, with pull-request code running only in `frontend-perf-measure` and the separate hash-verified `frontend-perf-gate` consuming its Criterion JSON as untrusted data, restoring the fresh main-only cache namespace but never publishing to it so repeated heads cannot ratchet or shadow the canonical comparison (D-046).

**D-047 was a temporary deferral, superseded by D-048 after PR-4 merged.**
D-048's exact-successful-main artifact gate established the required isolated
boundary and retired the earlier cache transport. D-051/D-053 now supersede
only its cross-run timing transport: every run measures the exact predecessor
and candidate sequentially on one hosted runner, seals the predecessor timing
before candidate code runs, and applies the unchanged greater-than-2% median
regression block through the predecessor-owned hash-verified comparator. The
D-048 activation variable, bootstrap branches, digest, and fixture are absent;
D-048 through D-050 remain historical rationale rather than live configuration.
PR-5 may proceed with the performance invariant already active.
This remains deliberately lightweight and distinct from the full
pyperformance/Nuitka/Codon/mypyc comparison suite (TESTING.md Layer 7), which
stays out of scope until v0.2.

D-051 records, and D-053 corrects, the active transport after repeated docs-only
CI runs proved that absolute estimates from two different hosted runners can
exceed the 2% threshold despite narrow within-run confidence intervals. The
exact predecessor and candidate are measured on one runner, and their paired
estimates are passed to a dedicated hash-verified median comparator inside the
same isolated review boundary. The active workflow binds the benchmark sources, root and
workspace manifests, local build scripts, lockfile, Rust toolchain, and Cargo
configuration; it seals the predecessor artifact before candidate code runs,
binds both downloads to the distinct artifact IDs returned by their trusted
upload steps, flattens each single-ID download into its own exact destination,
and requires any contract drift to use its own reviewed transition. Its active
bytes equal the reviewed D-051 fixture; only that digest is authorized. The
threshold, required `ci-gate` fan-in, and benchmark itself remain unchanged.

## Autonomy policy ("no questions" mechanics)

**Spec is law.** Where an existing doc already decided something, follow it literally. Where the spec is genuinely silent (as with the exact LLVM/inkwell version above), the agent picks the most conservative option that is *actually available in this environment* — never a hypothetical — records it as a new dated entry in [DECISIONS.md](./DECISIONS.md) using its existing template, and continues without stopping.

This resolves the one real tension with the merged `tdd` skill, which asks to confirm test seams with the user before writing tests: the approved plan (this document + the writing-plans output that follows it) enumerates the seams per crate up front. Approval of the plan **is** the seam confirmation; implementation agents do not re-ask.

This policy governs architectural/implementation forks only. Standing safety rules (force-push, branch deletion, rewriting published history, etc.) are unaffected and still pause for explicit confirmation.

## Delivery mechanics

This plan and its revisions are committed to their own short-lived branch off `main` and opened as a docs-only PR, reviewed before merge like any other change — review weight matched to risk, a read-through for docs rather than the full multi-agent pipeline reserved for compiler code. Every subsequent PR in the table above is its own feature branch off `main`, merged only once CI is green on all Tier-1 targets and that PR's slice of the v0.1 acceptance criteria is demonstrably met.

## Testing scope for v0.1

Of TESTING.md's 7 layers: Layer 1 (per-crate unit tests) from the start, Layer 3 (diagnostic snapshot tests) as soon as `pycc_diag` exists, Layer 5 (runtime property tests) in minimal form for the v0.1 runtime subset. **Out of scope for PR-1/PR-2**: Layer 2 (the real `pycc_testkit` conformance harness — deferred per D-018 to PR-4/PR-6, once there's a PEP matrix for it to check against; `tests/slice0.rs` covers the two named slice-0 fixtures ad hoc until then), Layer 4 (differential fuzzing), Layer 6 (OSS corpus), Layer 7's full cross-compiler comparison suite (vs. Nuitka/Codon/mypyc) — these start at v0.2 per ROADMAP.md. The lightweight per-PR frontend timing check is a different, narrower thing that *does* start in v0.1 — see Performance gate above (resolves #12).

**Debug/release conformance (resolves #10):** TESTING.md's conformance-harness rule ("compile `--debug` and `--release` both... flips to ✅ only when green on all Tier-1 targets in both profiles") describes the steady-state contract once `--release` exists. It does not apply yet: `--release`/LTO is a named v0.2 item (see the milestone table above), so for the whole of v0.1 the conformance harness runs `--debug` only, and no v0.1 PEP/feature is held to a `--release` bar that has nothing to build against. TESTING.md's wording is annotated accordingly rather than left to look like a v0.1 requirement no PR could actually satisfy.

Cutting across all of these: the D-014 coverage gate (`cargo llvm-cov --fail-under-lines 100 --fail-under-regions 100`) applies to every crate from PR-1 on — it is not a v0.1-specific item but a standing requirement, wired into the CI skeleton before any crate has a chance to accumulate untested code. Each task in the implementation plan below writes its test alongside its code for exactly this reason.

## Scope honesty

v0.1 is realistically a multi-PR, multi-session effort — real Rust/LLVM compilation and 5-target CI have minutes-scale iteration loops, not seconds. Work proceeds PR by PR, autonomously, opening each PR as it's ready, without stopping for questions, for as long as a session productively runs.
