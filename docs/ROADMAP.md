# pycc Roadmap

Milestone = shippable + demo-able. Acceptance criteria are binary; a milestone isn't done until they're green on **all Tier-1 platforms** (Linux x64/arm64, macOS x64/arm64, Windows x64).

## Current delivery status

Last reviewed on 2026-07-26. This section describes the repository tree in the
commit that contains it: behavior and evidence from that same commit count,
while work that exists only in another open pull request or unmerged branch
remains work in flight.

**Current milestone: v0.1 — in progress.** The first end-to-end vertical slice works on the primary macOS arm64 host, but v0.1 is not yet shippable.

| Area | Status in this commit | Evidence and remaining gap |
|---|---|---|
| Compiler pipeline | Frontend and backend both cover the full v0.1 surface, with documented gaps | The workspace contains the driver plus `pycc_ast`, `pycc_parser`, `pycc_hir`, `pycc_types`, `pycc_mir`, `pycc_codegen`, `pycc_rt`, and `pycc_diag`. `pycc check` runs the full parser → HIR → strict type checker pipeline, and `build`/`run` now compile the complete v0.1 language surface (arithmetic, comparisons, `if`/`while`/`for`+`range`, functions with real parameters/return values and recursion, `int`/`float`/`bool`/`str`, string concatenation, basic f-strings, and type-aware multi-argument `print`) through MIR → LLVM object → host linker → native executable, proven by [`tests/slice0.rs`](../tests/slice0.rs) and [`tests/slice1_codegen_depth.rs`](../tests/slice1_codegen_depth.rs). See the "Language surface" row for the specific known gaps this pipeline still panics on. |
| Language surface | v0.1 frontend and codegen both implemented, with documented gaps | `pycc check` accepts and type-checks the v0.1 grammar covered by this row (`build`/`run` implement the same coverage) -- arithmetic (including `int` overflow-to-bigint per D-001/D-058/D-061 and true-division-always-`float` promotion), comparisons (`int`/`float`/`bool`/`str`, lexicographic for `str`), `if`/`while`/`for`+`range`, functions/recursion, `str` (small-string optimization per D-007/D-059, concatenation, refcounted for named locals), basic f-strings, and `print` for every v0.1 scalar type. Known, accepted gaps (see D-061's own Consequences and Task 7/9's scope notes in [`docs/superpowers/plans/2026-07-25-pr5-codegen-depth.md`](./superpowers/plans/2026-07-25-pr5-codegen-depth.md)): comparing an already-promoted bigint `int`, `int` multiplication/floor-division/modulo/power on a bigint operand, `float` formatting outside CPython's own non-scientific-notation range, a `str` local not reassigned before its function returns, and unary operators (`-x`/`+x`/`not x`/`~x`) are all honest "not supported yet" boundaries, not silent wrong answers -- the last one is a *frontend* gap, not just a backend one: `Expr::UnaryOp` is never lowered in `pycc_hir::lower_expr` (shared by `check` and `build`/`run` alike), so `pycc check` panics on it too, confirmed while writing Task 11's `tests/slice1_codegen_depth.rs`. |
| Type system | Real v0.1 frontend subset | [`pycc_types::check_and_resolve`](../crates/pycc_types/src/lib.rs) enforces public annotations (`T0001`), rejects `Any` (`T0002`), infers and materializes local/private-helper signatures over the v0.1 primitive subset before MIR, classifies parameters plus nested assignment/`for` targets as function-local before resolving reads, applies Python true-division and `bool`-as-`int` rules, validates call/range/assignment compatibility, and rejects non-`None` functions when the conservative v0.1 control-flow analysis finds a fallthrough path. Function locals never fall back to same-named module globals before binding; #118 separately tracks definite-assignment joins. Later-language features such as containers, `Optional`, and narrowing have no grammar surface yet. |
| CLI | Partial | `build`, `run`, `version`, and frontend-only `check` have working paths, including validated `human` and `json` diagnostic formats; other `--error-format` values are rejected as bad invocation. `test`, `explain`, `init`, and `clean` still report “not yet implemented”; the broader flags and project-mode contracts in [CLI_SPEC.md](./CLI_SPEC.md) remain planned. |
| Diagnostics | Stable for the shipped frontend subset | Parser failures use `L0001`; public-signature, forbidden-`Any`, name-resolution (including unbound-local), operand/call/inference, return, assignment, and top-level-return failures use the registered `T0001`, `T0002`, `T0021`, `T0022`, `T0023`, and `T0024` codes with byte-exact checked-in CLI snapshots. The unbound-local regression has both human and version-1 JSON snapshots. Human and JSON output both carry primary labels. HIR/type diagnostics still use the documented line-1/column-1 placeholder span and do not yet populate safe `help:` suggestions (D-043). |
| Portability | Five-target Tier-1 matrix live | The PR-3 [CI run](https://github.com/rotnov/pycc/actions/runs/30135016242) builds and tests natively on all five Tier-1 targets (`ubuntu-latest`/x86_64, `ubuntu-24.04-arm`/aarch64, `macos-15-intel`/x86_64, `macos-14`/aarch64 via the coverage job, `windows-latest`) and cross-compiles `x86_64-apple-darwin` from the `macos-14` (arm64) runner, executed and verified on its native `macos-15-intel` runner. Cross-*OS* compilation (e.g. Linux/Windows from a Mac) remains a tracked future gap per D-026, not silently dropped. |
| Quality gates | Source-aware fixed-replicate performance gate active | Unit, diagnostic-snapshot, and slice-level end-to-end tests pass; CI enforces 100% Rust line and region coverage through `ci-gate`. The check-only type path validates the original HIR against inferred signatures without constructing the resolved clone needed only by downstream compiler stages, and per-function environments share the immutable module function registry through copy-on-write storage while keeping local bindings isolated. `frontend-perf-measure` and the isolated `frontend-perf-gate` remain required through `ci-gate`. Exact same-source-pair PR/main evidence reopened [#109](https://github.com/rotnov/pycc/issues/109) because changed-input single observations produced `+0.10%` and `+3.66%`. Active D-062 is byte-identical to the reviewed [source-aware five-replicate fixture](../tests/fixtures/d62-replicated-paired-ci.yml): D-056's complete identical-input classifier still makes timing visible non-blocking telemetry, while changed inputs use the median of five retained per-run medians and keep D-044's hard `>2%` block. Only D-062's digest is authorized; D-056 remains historical audit evidence. Revision, benchmark-contract, trust ordering, executable-input identity, artifact identity, exact ten-file evidence, and comparison drift fail closed. Activation is complete, but #109 stays open until repeated changed-source PR/main evidence validates the blocking path without result selection. The conformance harness, five-target language conformance, fuzzing, and corpus layers remain planned according to [TESTING.md](./TESTING.md). |
| Agent tooling | Project-local alpha; parity enforced | The reviewed `i-have-an-issue` research workflow and the alpha `pycc` / consent-gated `pycc-feedback` skills are discoverable in both Codex and Claude Code. D-066 adds the informational [`AGENT_RETROSPECTIVE.md`](./AGENT_RETROSPECTIVE.md) process-mistake journal and commit-grounded [`SESSION_LOG.md`](./SESSION_LOG.md) handoff snapshot: agents update them only at meaningful mistakes or checkpoints, review them for factual accuracy, privacy, and safe recovery, and promote any binding lesson into canonical policy, an ADR, or the owning specification. Agent-asset validation enforces the exact `CLAUDE.md` import of canonical `AGENTS.md` instructions plus skill-wrapper parity across both client entrypoints, and validates the immutable iEvo baseline plus reviewed skill provenance; required CI dispatches every declared scenario, including both compiler execution paths, the implemented strict-check diagnostic plus planned-`--fix` rejection, D-072's intentional frontend-pass/print()-nested-expression boundary, and offline research/consent contract checks. The gate also rejects required tracked assets that depend on optional unpinned Claude plugins, applies effective workflow/job/step and composite-action working directories recursively, follows local non-composite action entrypoints and interpreter stdin redirections, safely normalizes in-repository parent paths, recognizes bracketed IPv6 SCP sources, and matches optional GitHub marketplace coordinates with GitHub's case-insensitive owner/repository identity. Authenticated model-response evals remain a machine-enforced promotion requirement: without immutable evidence for both clients, the project alpha skills cannot enter the locked/published skill set. The public skills.sh page remains blocked on its stale index; [`vercel-labs/skills#1776`](https://github.com/vercel-labs/skills/issues/1776) tracks the external rescan. |
| Repository governance | Enforced | Protected `main` requires an up-to-date PR, the trusted `audit` context, resolved conversations, and the stable `ci-gate` context. That aggregate requires the coverage and Tier-1 build matrix plus the frontend measurement and isolated exact-predecessor comparison jobs. D-054 records the one-shot [#125 recovery](https://github.com/rotnov/pycc/issues/125) that merged staging [PR #119](https://github.com/rotnov/pycc/pull/119) while retaining every unaffected control; the app-bound `audit` plus `ci-gate` set was restored within seconds and grants no standing bypass. The required build runs clean-clone agent-policy validation, while the push-time [main-history audit](../.github/workflows/main-history-audit.yml) uses the pre-push checker (or its immutable reviewed bootstrap) and correlates each introduced commit with the merged-main PR whose merge commit arrived in that push. The external repository monitor verifies the push-controlled workflow and expected run; a changed workflow, missing run, or failure is release-blocking under [REPOSITORY_GOVERNANCE.md](./REPOSITORY_GOVERNANCE.md). |
| Public evidence and discoverability | Five-page publication package with measured discovery | [`site/`](../site/) contains the canonical landing page plus commit-relative status, implemented-versus-target architecture, a source-backed Python AOT compiler comparison, and the AI-native experiment. Every human- and LLM-readable project surface now reflects the same compiler boundary as this table: the broadened v0.1 frontend and strict checker are implemented through `pycc check`, and `build`/`run` now compile the complete v0.1 feature set through MIR, LLVM, and a real runtime, with the same documented gaps as the "Language surface" row above. The status page presents the required source-aware paired performance gate independently of the later compiler slice, and its negative validator mutations reject loss of same-runner exact revisions, sealed predecessor timing, executable-input identity, the conditional unchanged-input telemetry rule, the changed-source hard threshold, the fixed five-run plan, or the exact ten-file evidence set. The comparison covers LPython alongside Codon, Nuitka, mypyc, and Cython, distinguishes language, artifact, runtime, and current-positioning models using official project documentation, makes no benchmark claim, and visibly warns that pycc is not a production choice today. The website validator enforces the commit-relative frontend/backend facts, those sources, LPython's alpha positioning, and the warnings alongside unique metadata, connected structured data, internal navigation, visible authorship/status disclosures, the exact sitemap set, and the landing page's exact relative stylesheet plus deferred executable script references; independent negative mutations reject superseded compiler-status copy, missing LPython or other comparison evidence, missing paired performance evidence, non-wrapping narrow footer links, inert references, foreign-subtree asset tags, suppressing or execution-changing attributes, duplicate attributes, and base-URL overrides. Fresh-page browser QA at 320 and 390 CSS pixels confirms equal document and viewport widths across the landing, status, architecture, and comparison pages; qualified prose identifiers wrap, while wide tables retain local horizontal scrolling. The Pages workflow publishes the package from `main`; its best-effort IndexNow job batch-submits every canonical sitemap URL, and a hermetic HTTP fixture exercises the real notifier success and failure paths. [`SEARCH_VISIBILITY.md`](./SEARCH_VISIBILITY.md) preserves timestamped GitHub Search positions, rolling traffic snapshots, discovery-surface changes, and independent Search Console URL/sitemap/performance states without attributing automation-heavy activity to SEO. All five canonical website URLs now have positive Google URL Inspection evidence: the live `/python-aot-compilers/` comparison page was indexed by a successful mobile crawl with its declared canonical, valid HTTPS, and a valid breadcrumb after the four pre-existing URLs were confirmed. Search Console's processed performance report still contains 0 impressions, 0 clicks, and no query positions. Its unsuccessful sitemap processing remains a separate monitored observation from both the valid public sitemap and the individual indexing evidence. |

### v0.1 acceptance checklist

- [ ] `fib` and `mandelbrot-ascii` compile and match CPython output on all five Tier-1 targets.
- [ ] `pycc check` processes 1k LOC in under 50 ms.
- [ ] The error demonstration matches the stable [CLI specification](./CLI_SPEC.md) output.
- [x] The five-target native CI matrix and one cross-host compilation path are live on `main`. <!-- roadmap-evidence: ci-tier1-cross-compile -->
- [x] The 100% line and region coverage gate is required and green for the current slice. <!-- roadmap-evidence: ci-build-test-coverage-100 -->

Full v0.1 codegen/runtime breadth (this milestone's own codegen-depth slice, see the "Compiler pipeline"/"Language surface" rows above) is now implemented, with the documented gaps those rows list. The next delivery slices remain the sequence defined in [DELIVERY_PLAN.md](./DELIVERY_PLAN.md): the conformance testkit and named demos, then the final v0.1 acceptance pass.

## CPython release alignment

Last reviewed **2026-07-24**. D-012 still fixes v1 to the Python 3.14 language
level; patch releases advance the differential oracle without changing that
language-level decision.

| Track | Current upstream release | Roadmap action |
|---|---|---|
| v1 stable oracle | Python **3.14.6** final (2026-06-10) | Use 3.14.6 for new conformance recordings and the PR-6 CI oracle; keep all earlier 3.14 behavior green. |
| Next language level | Python **3.15.0b4** (2026-07-18), final beta | Track feature-frozen Final/Accepted standards in `PYTHON_STANDARDS.md`; adoption starts only after v1.0 and Python 3.15.0 final. |

Every newly observed Python release updates this table and
`PYTHON_STANDARDS.md` together. Observable maintenance-release semantics also
update the relevant runtime/stdlib spec and differential fixtures.

## v0.1 — "hello, binary"

Functions, `int`/`float`/`str`/`bool`, arithmetic, comparisons, `if`/`while`/`for`+`range`, f-strings (basic), `print`, module-level code, recursion. Frontend: strict annotations (`T0001`), local inference. Backend: LLVM debug builds, vendored parser allowed.

**Accept:** `fib`, `mandelbrot-ascii` compile & match CPython output on 5 targets; `pycc check` on 1k LOC < 50 ms; error demo screenshot-parity with CLI_SPEC.md; CI matrix live; 100% line+region coverage gate green on every crate (D-014, see TESTING.md).

## v0.2 — collections & generics

`list`/`dict`/`set`/`tuple` + literals, comprehensions, slicing, methods; PEP 585/695 generics via monomorphization; `--release` profile (LTO); `pycc.toml`.

**Accept:** corpus Tier-1 (`tomli`, `packaging`, `more-itertools`) compiles; nbody ≥ 20× CPython; conformance ≥ 25 PEPs green.

## v0.3 — classes & pattern matching

Classes, inheritance+C3, `@property`, dataclasses, enums, protocols, `match` (634) with exhaustiveness, exceptions (`try/except/finally`, chains).

**Accept:** conformance ≥ 45 PEPs; diagnostics registry fully implemented for shipped features; `pycc explain` live.

## v0.4 — projects & incremental

Multi-file, imports, namespace packages (420), incremental cache, parallel codegen, `os`/`pathlib`/`json`/`datetime` native.

**Accept:** corpus Tier-2 (`black`, `isort`, `attrs`, `click`) ≥ 80% files compile; incremental rebuild of 10k LOC < 200 ms; cross-compile demo mac→windows.exe in README gif.

## v0.5 — generators & ownership v1

Generators/`yield from` as state machines, iterator protocol, `itertools`/`functools`; ownership: escape analysis + move semantics + RC elision live; `--memstats`.

**Accept:** RC-elision ≥ 70% on corpus mean; fuzzing layer-4 running continuously; zero known miscompiles open > 7 days.

## v0.6 — threads without GIL

`threading`/`queue`, Shareable/move checks (`O03xx`), cycle collector, own parser replaces vendored (D-003 resolved).

**Accept:** thread-safety negative tests; 8-core scaling demo ≥ 6× on embarrassingly-parallel bench; race detector (TSan CI job) clean.

## v0.7 — interop escape hatch

`pycc.interop.cpython`, typed boundary (`I04xx`), `[interop] allow` config; `unittest`/`logging`/`argparse`.

**Accept:** demo: compiled app calls numpy through the hatch; boundary-cost benchmark published; pure-mode (`allow = []`) guarantees no libpython dependency.

## v0.8 — corpus at scale + bot

Corpus Tier-3 (`mypy`, `httpx`, `rich`) tracked; corpus-bot auto-issues live; `socket`/`http.client`; compression stack incl. PEP 784 zstd.

**Accept:** bot files/dedupes/closes issues autonomously for 30 days without human cleanup; ≥ 90 PEPs green.

## v0.9 — async & packaging

`asyncio` subset on state machines; `pycc build --lib` C-ABI; binary size diet; signing/notarization docs per OS.

**Accept:** async echo-server demo; `--lib` consumed from Rust and C in CI.

## v1.0 — spec freeze

PYTHON_STANDARDS Python 3.0–3.14 matrix: every row ✅ or explicitly
`rejected-by-design` with negative test; the Python 3.15 preview rows do not
gate v1.0. Semantics deviations doc complete; benchmarks vs
CPython/Nuitka/Codon/mypyc published; diagnostics/JSON formats frozen (semver).

**Accept:** corpus Tier-1..3 green 3 releases in a row; fuzzer finds 0 mismatches for 30 consecutive days; docs site.

## v1.x — Python 3.15 adoption

Starts only after both pycc v1.0 and upstream Python 3.15.0 final. Add the 3.15
grammar, typing rules, import behavior, builtins, and stdlib surface listed in
the preview matrix while preserving Python 3.14 compatibility.

The gate-opening change must introduce a machine-readable supported-language
registry consumed by project-configuration validation and conformance CI. For
each supported level, that registry binds the accepted configuration value,
cumulative fixture range, and pinned CPython oracle; CI must verify that the
documented matrix and executable conformance jobs agree with it. Until that
change lands, the 3.15 rows remain planning inputs rather than
supported-language claims.

**Accept:** every Python 3.15 matrix row is ✅ or explicitly
`rejected-by-design` with a negative test; differential conformance runs
the cumulative Python 3.0–3.15 fixture set against a pinned current Python 3.15
patch on all Tier-1 targets; the independent Python 3.14 configuration keeps
the complete Python 3.0–3.14 suite green; a new ADR supersedes D-012 and
records the supported-version policy; the supported-language registry and its
matrix/oracle consistency check are required and green.

## Post-1.0 (parking lot)

LSP server · `wasm32-wasi` target · PGO/BOLT pipeline · Cranelift debug backend (D-002) · pip-installable wheels of compiled modules · REPL via cranelift-jit (yes, ironic).
