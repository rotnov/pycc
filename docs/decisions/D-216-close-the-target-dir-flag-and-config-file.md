---
id: D-216
title: "Close the --target-dir flag and config-file build.target-dir exclusions as permanent"
status: accepted
---

## D-216: Close the `--target-dir` flag and config-file `build.target-dir` exclusions as permanent

- Status: accepted (issue
  [#639](https://github.com/rotnov/pycc/issues/639), filed by
  [D-183](./D-183-honor-cargo-target-dir-when-locating-build.md) as its own
  named follow-up). **Narrows** D-183 by resolving the one open question its
  *Decision* item 3 and `docs/CLI_SPEC.md`'s Environment section both left
  pending, and **narrows**
  [D-184](./D-184-build-pycc-rt-from-pycc-codegen-s-build.md) in the same
  way: D-184's *Consequences* deferred to #639 the write-side effect of the
  same two excluded inputs on `pycc_codegen`'s build script. Neither entry
  is superseded.

- Context:

  D-183 gave `pycc build` and its test helpers a single resolver,
  `pycc_artifact_layout::resolve_cargo_target_root`, that honors
  `CARGO_TARGET_DIR` then `CARGO_BUILD_TARGET_DIR` (Cargo's environment
  mapping of the `build.target-dir` config key) then a manifest-relative
  fallback. It deliberately left two higher-precedence Cargo inputs
  unconsulted — the `--target-dir` command-line flag and a `build.target-dir`
  key set in a `.cargo/config.toml` file — and filed #639 to decide,
  separately, whether either should be closed instead of left open. #639's
  own first completion criterion names the legitimate third outcome:
  "neither; the environment-variable precedence is the whole contract",
  which closes the issue by making the exclusion permanent rather than
  pending.

  Nothing about either exclusion's technical basis has changed since D-183
  measured it, and re-verifying against the current tree confirms both still
  hold:

  1. **The config-file form.** Honoring `build.target-dir` set in
     `.cargo/config.toml` means re-implementing Cargo's own config
     discovery: the ancestor walk from the invocation directory upward, the
     `$CARGO_HOME` merge, and Cargo's own precedence rules between them.
     `pycc_artifact_layout::resolve_cargo_target_root` (`crates/pycc_artifact_layout/src/lib.rs`)
     is a pure function over an already-resolved `manifest_dir` and an
     injected `env_lookup`; none of that discovery machinery exists in this
     workspace today, on either side of the resolver. Building it is a
     materially larger surface than reading two environment variables, and
     every branch of the walk and the merge would need an executing test
     under this project's 100%-line-and-region gate (D-014). That cost is
     out of proportion to the gap, and — as D-183's item 3 already states —
     the argument rests on no claim about what Cargo exports: D-183 measured
     that `CARGO_BUILD_TARGET_DIR` is honored by Cargo when a caller sets it
     as an environment variable, and the config-file form is precisely the
     case that mapping does *not* cover. A user who redirects through
     `.cargo/config.toml` and wants `pycc` to follow has an exact,
     documented alternative that costs nothing to support: export
     `CARGO_TARGET_DIR` (or `CARGO_BUILD_TARGET_DIR`) to the same path.

  2. **The `--target-dir` flag.** Its resolved path is reachable from an
     *integration-test or bench binary* only through the compile-time
     `CARGO_TARGET_TMPDIR` macro (`env!("CARGO_TARGET_TMPDIR")`), never
     through any runtime environment variable, and the macro is not set for
     `bin` targets (D-183's own measurement), build scripts, or a library
     crate's own `#[cfg(test)]` unit-test binary — so it never reaches the
     `pycc` driver a user actually invokes. The last two negatives were
     measured for this entry rather than reasoned out, following D-183's
     recorded lesson: in a throwaway package under
     `cargo test --target-dir <dir>` — run outside this repository tree, so
     the package was probed with both the ambient cargo 1.88.0 and, via
     `cargo +1.97.1`, this project's pinned toolchain, with identical
     results — `option_env!("CARGO_TARGET_TMPDIR")` (rather than D-183's
     `env!`, which fails to compile in exactly the contexts where the answer
     is `None`) is `Some("<dir>/tmp")` in
     the integration-test binary, `None` in the library's `#[cfg(test)]`
     unit-test binary, and `None` in `build.rs` at both compile time and
     runtime. `resolve_cargo_target_root` is the one resolver
     shared by four call sites in the current tree — the driver,
     `pycc_codegen`'s link-and-run tests (a `#[cfg(test)]` module of the
     library crate), `tests/slice0.rs`'s skip guards, and `pycc_codegen`'s
     own build script (`crates/pycc_codegen/build.rs`, added by D-184) —
     and the macro is available to exactly one of them, `tests/slice0.rs`;
     anchoring the resolver on it would give the same function two
     different resolution rules depending on which binary it was compiled
     into, which is worse than the status quo of not consulting the flag at
     all. Nothing observes `--target-dir` in the `pycc` process itself —
     Cargo has no documented plan to export it at runtime, and this project
     has no interposing wrapper of its own invocation. The one candidate
     mechanism is indirect: a build script's `OUT_DIR` lives inside the
     actual target root, so that root could be derived from it and embedded
     into the binary with `cargo::rustc-env`. It is evaluated and rejected
     under *Alternatives* below.

- Decision:

  Neither the `--target-dir` command-line flag nor a config-file
  `build.target-dir` will be honored. The environment-variable precedence
  chain D-183 established — `CARGO_TARGET_DIR`, then
  `CARGO_BUILD_TARGET_DIR`, then the manifest-relative fallback — is the
  complete and permanent contract for locating Cargo-produced build
  artifacts. This closes #639: it is not a deferral awaiting a future
  implementation, it is the considered answer.

  No behavioral code changes accompany this decision.
  `resolve_cargo_target_root` already implements exactly this three-level
  chain and already declines to read either excluded input — there is no
  config-discovery code or compile-time-macro wiring to remove, because
  none was ever added. The work this decision closes is documentary:
  replacing every "deferred, tracked as #639" cross-reference with a
  statement that the exclusion is final, so a future reader does not treat
  #639 as reopenable scope. This entry updates `docs/CLI_SPEC.md`'s
  Environment section, `docs/ROADMAP.md`'s CLI row, and the
  `resolve_cargo_target_root` rustdoc in
  `crates/pycc_artifact_layout/src/lib.rs` (a doc-comment citation only,
  so the pointer readers follow from the resolver itself reaches this
  entry); D-183's and D-184's own #639 references are left as historical
  record, because an accepted decision is not rewritten (see
  *Consequences*).

  The same answer covers the write side. D-184's build script
  (`crates/pycc_codegen/build.rs`) installs the `pycc_rt` archives at the
  root `resolve_cargo_target_root` returns, so under a `--target-dir` flag
  or a config-file `build.target-dir` — with neither environment variable
  set — it installs into the `<workspace>/target` fallback while Cargo
  builds elsewhere. D-184 recorded that stray install as new and deferred
  it to #639; this decision accepts it as permanent. It is the read-side
  exclusion's necessary mirror image — one resolver, one rule — and its
  cost is bounded to a stray `<workspace>/target` tree beside the
  redirected one: because the driver resolves the same fallback root the
  build script installed into, the two agree on where the archive is, and
  nothing mislinks.

  Whenever the archive is nevertheless absent from the resolved root —
  the build script has not run for the profile or `--target` triple in
  use, or the artifact was cleaned — a build driven through either excluded
  input keeps the exact behavior D-183 already gives it: the ordinary
  actionable `no pycc_rt build found (expected …)` message, not a silent
  mislink. That message names the directory the resolver actually searched,
  which remains the right diagnostic regardless of why the artifact isn't
  there.

- Alternatives:

  - **Implement the config-file form.** Rejected for the coverage-cost
    reason above, unchanged from D-183's own reasoning: the ancestor walk
    and `$CARGO_HOME` merge are Cargo behavior this project has never had
    to reproduce anywhere else, and the user who needs the redirect
    followed already has a zero-cost, documented equivalent in exporting
    `CARGO_TARGET_DIR` to the same path.

  - **Implement the flag by keying resolution on the target kind (`bin` vs.
    integration-test/bench).** Rejected: it does not close the gap for the
    binary that matters most. `pycc`'s own driver is exclusively a `bin`
    target, so a kind-keyed resolver would still never honor `--target-dir`
    for the `pycc build`/`pycc run` invocations a user actually runs — it
    would only add a second resolution rule to cover the single
    integration-test call site (`tests/slice0.rs`) that already works
    correctly today via `CARGO_TARGET_DIR`. Two rules for one function, in
    exchange for coverage of a path that was never the gap users hit.

  - **Derive the target root from a build script's `OUT_DIR` and embed it
    with `cargo::rustc-env`.** Raised in review of this entry, and it is a
    real mechanism: `OUT_DIR` is
    `<root>/[<triple>/]<profile>/build/<pkg>-<hash>/out`, the root
    `build.rs` already exports build-time values this way
    (`PYCC_BUILD_RUSTC_VERSION`), and `pycc_codegen`'s build script already
    reads `OUT_DIR` (`anchor_target_root_for_build_script`). Rejected on
    four grounds. First, it is the kind-keyed alternative above in another
    costume: a `cargo::rustc-env` value is scoped to the targets of the
    package whose build script emits it, and `resolve_cargo_target_root`
    lives in `pycc_artifact_layout`, which has no build script — so the
    shared resolver could never read the value itself. Every consuming
    package (`pycc`, `pycc_codegen`) would need its build script to emit
    its own constant and its own call sites to thread that constant in,
    which is N per-package anchors in place of one shared rule. Second, it
    depends on the
    internal target-directory layout, which Cargo documents as unstable
    (the `build/<pkg>-<hash>/out` shape is described, not promised).
    Third, it freezes a per-invocation absolute path into the binary — a
    strictly more volatile anchor than `CARGO_MANIFEST_DIR` — and regresses
    `cargo install`, whose temporary target directory is deleted after the
    install, leaving the embedded root pointing at nothing. Fourth,
    `anchor_target_root_for_build_script`'s `diverged` warning exists
    precisely because anchoring a *relative* resolved root against the
    workspace root — one corner of the `OUT_DIR`-versus-resolved-root
    relationship — already proved sharp enough to need a guardrail. Against all that, the
    exclusion costs a stray `<workspace>/target` tree and nothing else,
    because both sides resolve the same root.

  - **Leave #639 open as a standing deferral instead of closing it.**
    Rejected: #639's own completion criteria list "neither" as a valid
    closing outcome precisely so that a considered "no" does not have to
    masquerade as unfinished work. An open issue with no planned
    implementation and no blocking dependency is tracker noise, not a
    signal of remaining scope.

- Consequences:

  #639 closes. `docs/CLI_SPEC.md`'s Environment section and
  `docs/ROADMAP.md`'s CLI row both state the exclusion as permanent instead
  of pending, citing this entry instead of the issue. D-183 itself is left
  unedited: it is an accepted decision recording what was true and reasoned
  at the time it was written (including its own explicit statement that
  #639 "carries both exclusions and their reasoning" and "its own first
  completion criterion admits the outcome that neither form is ever
  honored"), and this entry is the promised resolution rather than a
  correction to it. D-184 is likewise left unedited: its *Consequences*
  bullet stating that #639 "tracks whether either input should be honored
  at all" now resolves here, and the write-side stray-install behaviour it
  describes — the build script populating `<workspace>/target` while Cargo
  builds under the redirected directory — is accepted as permanent by this
  decision. A future contributor who wants either input honored
  after all needs a new decision that supersedes this one with a changed
  premise — a new call site that needs the config-file form, or a Cargo
  release that exposes `--target-dir` to `bin` targets at runtime — not a
  reopening of #639.
