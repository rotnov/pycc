---
id: D-183
title: "Honor CARGO_TARGET_DIR when locating build artifacts"
status: accepted
---

## D-183: Honor CARGO_TARGET_DIR when locating build artifacts

- Status: accepted (issue
  [#629](https://github.com/rotnov/pycc/issues/629)). **Narrows** one
  derivation claim in
  [D-091](./D-091-relax-frontend-perf-measure-s-exact-manifest.md) rather
  than superseding it; see *Consequences*.

- Context:

  `pycc build` links the object it compiles against `pycc_rt`'s static
  library, which it does not build itself — Cargo does, and Cargo puts it
  in the *Cargo target directory*. Every lookup of that library in this
  workspace assumed the target directory is `<workspace root>/target`,
  spelled as an `env!("CARGO_MANIFEST_DIR")`-relative join in three
  places: the compiler driver's `find_pycc_rt_lib_dir_in` (`src/main.rs`),
  `pycc_codegen`'s own link-and-run test helper
  `link_object_with_runtime`, and two `tests/slice0.rs` skip guards.

  That assumption is wrong whenever `CARGO_TARGET_DIR` is set, which is
  ordinary Cargo usage (shared target directories across checkouts, CI
  caches, sandboxed builds). The failure modes differ by site and the
  quiet one is the worse one: the driver reports "no pycc_rt build found"
  naming a directory Cargo never wrote to, while a skip guard silently
  decides the artifact is absent and skips the test it guards — a pass
  that measured nothing, which D-014's coverage gate then counts as an
  uncovered region rather than a green test.

  This repository already runs with a redirected target directory: the
  `build-test-coverage` job sets `CARGO_TARGET_DIR=$ISOLATED_ROOT/target`
  inside its `nobody` sandbox and separately symlinks
  `$GITHUB_WORKSPACE/target` at the same tree, so the manifest-relative
  literals happen to resolve. That coincidence is the reason the bug
  survived: the workspace-relative path is correct there only because a
  symlink was placed to make it correct, not because the lookup is right.

- Decision:

  1. A single resolver,
     `pycc_codegen::artifact_layout::resolve_cargo_target_root`, owns the
     rule. Three levels, keeping Cargo's own relative order:
     `CARGO_TARGET_DIR` when set to a non-empty value; otherwise
     `CARGO_BUILD_TARGET_DIR` when set to a non-empty value; otherwise
     `<manifest dir>/target`. The second level is Cargo's generic
     config-to-environment mapping of the `build.target-dir` config key,
     and Cargo honors it with no `.cargo/config.toml` present at all —
     measured on this project's dev host rather than assumed:
     `CARGO_BUILD_TARGET_DIR=<dir> cargo build` writes to `<dir>/debug`
     and creates no local `target/`, and with both variables set
     `CARGO_TARGET_DIR` wins while the `CARGO_BUILD_TARGET_DIR` path is
     never created.

     An empty value at either level is treated as unset. This
     **diverges** from Cargo deliberately: Cargo does not fall back, it
     aborts (`CARGO_TARGET_DIR= cargo build` exits 101 with "the target
     directory is set to an empty string in the `CARGO_TARGET_DIR`
     environment variable"). A compiler driver searching for an artifact
     someone else built has no equivalent abort to offer, honoring the
     empty string would resolve artifacts to a bare relative `debug/`,
     and an exported-but-empty variable is a common shell accident rather
     than an intent to redirect — so falling back is both safer and
     closer to the user's evident intent.

     A relative value is passed through unchanged rather than re-anchored
     on the manifest directory. Cargo resolves a relative target
     directory against the working directory of the process that invoked
     *it*; `pycc` is a separate process, so passing it through agrees
     with Cargo exactly when `pycc` runs from that same directory, and
     re-anchoring on the manifest directory would agree with Cargo in no
     case at all.

  2. Every artifact lookup goes through it. `pycc_rt_lib_filename` and
     `find_pycc_rt_lib_dir_in` move from `src/main.rs` into the same
     public `artifact_layout` module so the compiler driver,
     `pycc_codegen`'s link-and-run tests, and `tests/slice0.rs`'s skip
     guards share one implementation and one coverage denominator.
     `find_pycc_rt_lib_dir_in`'s first parameter becomes an
     already-resolved `target_root`; it no longer joins `"target"`
     itself. The `exists` dependency-injection point stays a plain `fn`
     pointer and `env_lookup` is introduced as a second one, for the
     reason recorded on `exists`: `impl Fn` would monomorphize one body
     per closure type, each exercising only the arm its caller takes,
     which reads as a real region gap under `cargo llvm-cov`.

  3. Two inputs rank above both variables in Cargo's own precedence and
     are **deliberately not** consulted: the `--target-dir`
     **command-line flag**, and `build.target-dir` when set in a
     `.cargo/config.toml` **config file**.

     The config-file form is exactly the case the
     `CARGO_BUILD_TARGET_DIR` mapping in item 1 does *not* cover, and
     honoring it means re-implementing Cargo's config discovery inside
     `pycc`: walking ancestor `.cargo/config.toml` files from the
     invocation directory, merging with `$CARGO_HOME/config.toml`, and
     applying Cargo's own precedence and path-resolution rules. That is a
     materially larger surface than this issue's behavioral gap, and
     every branch of it would need deterministic tests to satisfy
     D-014's 100% line-and-region gate. That reason alone is sufficient
     and rests on no claim about what Cargo exports.

     The flag is deferred for a different and narrower reason, measured
     rather than reasoned out. Its resolved path *is* reachable from a
     compiled binary, but only through the **compile-time**
     `CARGO_TARGET_TMPDIR` macro: under `cargo test --target-dir <dir>`,
     `env!("CARGO_TARGET_TMPDIR")` evaluates to `<dir>/tmp`, while a
     runtime `std::env::var` of `CARGO_TARGET_TMPDIR`, `OUT_DIR`,
     `CARGO_TARGET_DIR`, and `CARGO_BUILD_TARGET_DIR` in that same binary
     all return `NotPresent`. And the macro is not set for `bin` targets
     at all, so it never reaches the `pycc` driver a user invokes — no
     Cargo anywhere in that process's ancestry. Anchoring this one shared
     resolver on it would therefore give the same function two different
     resolution rules depending on which binary it was compiled into:
     one for `tests/slice0.rs` and `pycc_codegen`'s link-and-run tests,
     another for the driver. A single rule that all three obey is worth
     more than covering the flag for two of them. Deferred rather than
     half-implemented here;
     the documented precedence in `docs/CLI_SPEC.md` states plainly which
     inputs are honored today. The deferral is tracked as
     [#639](https://github.com/rotnov/pycc/issues/639), filed with this
     change; that issue carries both exclusions and their reasoning, and
     its own first completion criterion admits the outcome that neither
     form is ever honored and the environment-variable precedence is the
     whole contract.

     Both exclusions above were narrowed by review, in two rounds, and
     the history is recorded rather than smoothed over because the same
     mistake recurred. Round 2: the original text asserted that neither
     input "is visible to a compiled binary through any environment or
     argument Cargo exports", which Cargo's generic
     config-to-environment mapping (`build.target-dir` →
     `CARGO_BUILD_TARGET_DIR`) makes false for half of it — a user
     redirecting artifacts that way hit exactly the `no pycc_rt build
     found` failure this decision exists to remove, while the docs
     asserted the scenario could not arise. That round's own recorded
     lesson was "a claim about what another tool exports is a claim to
     measure, not to reason out". Round 3 then caught that the
     replacement clause, "neither reaches a compiled binary", had been
     reasoned out too: `CARGO_TARGET_TMPDIR` carries the
     `--target-dir`-resolved path into precisely the test binaries this
     decision's *Consequences* brings under the shared resolver. The
     lesson survived its own restatement only by being applied a second
     time; the wording above now cites the measurement instead of an
     inference, and a future narrowing should do the same.

  4. A mechanical gate, `tests/target_dir_literals.rs`, forbids
     reintroducing the literal shape. It is a Rust `#[test]`, not a CI
     step, on purpose: `.github/workflows/ci.yml` is a
     manifest-protected path under D-103's staging rules, so a CI-step
     gate would force a two-PR stage-then-activate cycle for a check that
     a workspace test enforces identically through the existing required
     `cargo test` and coverage jobs. It scans an explicit `src`,
     `crates`, `tests`, `benches` allowlist rather than walking the
     workspace root — under the coverage job the root holds the `target`
     symlink into build output — and does not shell out to `git
     ls-files`, since that job runs as `nobody` under `env -i` with no
     `git` on `PATH`. Its needles are assembled with `concat!` so the
     gate does not match its own source.

- Alternatives:

  - **Walk `current_exe()`'s ancestors to find the target directory.**
    Rejected, and measured before rejecting: under `cargo llvm-cov` the
    test binaries live in `target/llvm-cov-target/debug/deps/`, so an
    ancestor walk lands in the wrong subtree, while `CARGO_TARGET_DIR`
    is left intact by `llvm-cov` and resolves correctly. The
    environment variable is the more reliable signal in exactly the
    environment that matters most here.

  - **Allowlist the surviving `target/debug`-shaped literals instead of
    rewriting them.** Rejected: an allowlist preserves three independent
    copies of the wrong rule and turns the gate into a record of
    exceptions. Each literal has a correct replacement that is no longer
    to write.

  - **Put the shared resolver in a new crate, or in `pycc_rt`.**
    Rejected: `pycc_codegen` is already a dependency of the `pycc`
    package (so its integration tests and benches can reach it) and of
    nothing that would be burdened by the module. A new crate adds a
    workspace member, a manifest, and a dependency edge for three
    functions; `pycc_rt` is the compiled program's runtime and must not
    grow build-host concerns.

  - **Keep the CI symlink as the mechanism and document it.** Rejected:
    it makes correctness a property of one job's setup rather than of
    the code, and does nothing for a developer with `CARGO_TARGET_DIR`
    exported in their shell — the reported case.

- Consequences:

  Setting `CARGO_TARGET_DIR` or `CARGO_BUILD_TARGET_DIR` now works for
  `pycc build`, for `pycc_codegen`'s link-and-run tests, and for the
  cross-target skip guards, in every environment rather than only where a
  symlink happens to paper over the difference. The `--target-dir` flag
  and a config-file `build.target-dir` remain unhonored and are stated as
  such in `docs/CLI_SPEC.md`; a build driven through either, with neither
  environment variable set, still fails with the existing actionable "no
  pycc_rt build found (expected …)" message rather than silently
  mislinking. Closing that gap is a follow-up that no issue tracks yet;
  see the *Decision* item above.

  This narrows exactly one sentence of D-091. D-091's item 4 justified
  adding the sandbox's release-profile `pycc_rt` build by deriving that
  `CARGO_TARGET_DIR` plus the `ln -s "$ISOLATED_ROOT/target"
  "$GITHUB_WORKSPACE/target"` symlink land the library "exactly [at] the
  `env!("CARGO_MANIFEST_DIR")`-relative path `find_pycc_rt_lib_dir_in`
  resolves to". That derivation was accurate when written and its
  conclusion — the build step is needed and lands the library where the
  lookup looks — is unchanged and still accepted. What changes is why:
  the lookup now reads `CARGO_TARGET_DIR` directly, so the symlink is no
  longer load-bearing for artifact resolution. Nothing else in D-091 is
  affected, and the symlink itself is left in place: it is
  manifest-protected `ci.yml` content, other steps may rely on it, and
  removing it is not this decision's business.
