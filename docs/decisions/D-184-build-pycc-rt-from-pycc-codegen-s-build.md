---
id: D-184
title: "Build pycc_rt from pycc_codegen's build script"
status: accepted
---

## D-184: Build pycc_rt from pycc_codegen's build script

- Status: accepted (issue
  [#630](https://github.com/rotnov/pycc/issues/630), Part 2 of
  [#20](https://github.com/rotnov/pycc/issues/20)). **Amends**
  [D-183](./D-183-honor-cargo-target-dir-when-locating-build.md) on where
  the resolver lives, and supersedes a position recorded in
  `crates/pycc_rt/src/lib.rs`; both are named below.

- Context:

  `pycc_rt`'s `staticlib` output is what every pycc-generated object file
  links against, but Cargo never produces it as a side effect of building
  anything that needs it. Cargo does not uplift a `staticlib` reached
  through an ordinary `[dependencies]` edge to a predictable path — only
  `<root>/<profile>/deps/libpycc_rt-<hash>.a` exists, under a hash nothing
  downstream can compute — and `cargo test` does not build the `staticlib`
  crate type at all. A clean checkout therefore fails its own tests until
  someone runs `cargo build -p pycc_rt` by hand, which is exactly the
  breakage #20 exists to remove. Artifact dependencies
  (`pycc_rt = { path = "...", artifact = "staticlib" }`) express this
  directly but require `-Z bindeps`, which this project's pinned
  `rust-version = "1.97.1"` cannot use.

- Decision:

  `pycc_codegen` gains this tree's first `build.rs`. It runs
  `cargo build --locked -p pycc_rt` for **both** host profiles into a
  private directory under its own `OUT_DIR`, then installs each resulting
  archive at the D-183-resolved target root, in Cargo's own layout
  (`<root>/<profile>/`, or `<root>/<triple>/<profile>/` when
  cross-compiling). Both profiles are built unconditionally because which
  one gets linked is chosen at `pycc build --release` time, long after the
  build script has run, and `PROFILE` describes only how `pycc_codegen`
  itself is being compiled.

  Ten properties of that decision are recorded here because each one was
  weighed rather than fallen into:

  **(i) The build script writes outside `OUT_DIR`.** This is a deliberate
  documented deviation from Cargo convention. It is justified because the
  destination is not an arbitrary directory: it is the exact root
  `pycc_artifact_layout::resolve_cargo_target_root` already resolves and
  every consumer already reads — the compiler driver, `pycc_codegen`'s
  link-and-run tests, and `tests/slice0.rs`'s skip guards. Writing only
  inside `OUT_DIR` would leave the archive at a path nothing can find.

  **(ii) The nested build gets its own `--target-dir`.** A build script
  that invokes `cargo` at the *same* build directory blocks indefinitely
  on Cargo's build-directory lock, which the outer invocation holds for
  the whole build-script execution; this was measured (>45 s, no
  progress), and it is the failure that made an earlier attempt look
  impossible. `OUT_DIR/pycc_rt-nested-<profile>` is private to this
  build unit, so no lock is shared. `pycc_rt` declaring **no dependencies
  at all** is the second half of the argument: the nested invocation needs
  no registry access, so it never contends for
  `$CARGO_HOME/.package-cache` either, and `--locked` cannot fail on a
  dependency the lockfile does not describe.
  `tests/issue_630_pycc_rt_build_dependency.rs` asserts that property so
  it cannot be lost silently.

  **(iii) The nested environment is scrubbed by namespace, not rebuilt
  from an allowlist.** Every variable for which
  `pycc_artifact_layout::should_scrub_for_nested_cargo` returns true is
  removed: everything beginning `CARGO_` (except `CARGO_HOME`, which names
  the shared registry rather than anything about the current build) or
  `RUSTC_`, plus `RUSTFLAGS`, `RUSTDOCFLAGS`, and `LLVM_PROFILE_FILE`.
  Bare `CARGO` survives — it names the cargo binary to re-invoke — and so
  does `RUSTC`, which does not begin `RUSTC_`; both are intentional, not
  oversights of the prefix rule.

  The rejected shape was `Command::env_clear()` plus an explicit
  allowlist. It was measured to work on macOS and rejected anyway:
  Windows MSVC is a Tier-1 CI leg, an MSVC toolchain needs `INCLUDE`,
  `LIB`, and `LIBPATH` (among others) to function, and the correct
  Windows allowlist is not verifiable from this project's single
  development host. A removal predicate fails safe in the other
  direction — an unanticipated platform variable survives instead of
  vanishing.

  The reason `RUSTFLAGS` and `LLVM_PROFILE_FILE` must go is that the
  runtime has to stay **un-instrumented**: under `cargo llvm-cov` the
  outer build exports coverage instrumentation flags and a profile-output
  template, and an instrumented `libpycc_rt.a` would be linked into every
  compiled test program, emitting stray `.profraw` files and inflating the
  coverage denominator with a crate the gate does not measure that way.
  The accepted cost is that a user's **legitimate** `RUSTFLAGS` (say
  `-C target-cpu=native`) does not reach the nested runtime build. That is
  a real loss, taken knowingly: there is no way to distinguish a user's
  flags from the harness's from inside the build script, and silently
  instrumenting the runtime is the worse failure.

  **(iv) The resolver becomes its own crate — an amendment of D-183.**
  A crate cannot depend on itself, so `pycc_codegen`'s build script cannot
  use `pycc_codegen`'s own module. `artifact_layout.rs` therefore moves
  verbatim into a new dependency-free workspace member,
  `crates/pycc_artifact_layout`, which `pycc_codegen` takes under **both**
  `[dependencies]` and `[build-dependencies]`.

  D-183 records the resolver's home in two places —
  `docs/decisions/D-183-honor-cargo-target-dir-when-locating-build.md:46`
  ("A single resolver, `pycc_codegen::artifact_layout::resolve_cargo_target_root`,
  owns the rule.") and `:80` ("`pycc_rt_lib_filename` and
  `find_pycc_rt_lib_dir_in` move from `src/main.rs` into the same public
  `artifact_layout` module"). Per AGENTS.md an accepted decision is
  superseded by a new one and never silently rewritten, so those two
  sentences are named here rather than edited there: the resolver now
  lives in `pycc_artifact_layout`.

  Everything else in D-183 — including the *paths* those sentences use —
  stays literally true, because `pycc_codegen` re-exports the crate as
  `pub use pycc_artifact_layout as artifact_layout;`. Every existing
  `pycc_codegen::artifact_layout::…` path still resolves, which is why
  this move obliges no edit to `docs/TESTING.md` or to any other document
  that names one.

  **(v) The build script anchors a relative target root; the runtime
  resolver still passes one through.** These deliberately disagree, and
  D-183's own rationale is why. Cargo resolves a relative target directory
  against the working directory of the process that invoked *it*; `pycc`
  at runtime is a separate process, so passing the value through agrees
  with Cargo exactly when `pycc` runs from that directory and re-anchoring
  would agree in no case. A build script has no such freedom — Cargo runs
  it with an unspecified working directory — so a bare relative root is
  not usable as written, and anchoring on the workspace root is the only
  interpretation available. It is correct whenever cargo was invoked from
  the workspace root, which is the common case and the one CI uses.

  The residual case is detected rather than assumed away.
  `OUT_DIR` is always inside the target directory Cargo is *really* using,
  so when the anchored root does not contain `OUT_DIR` the guess was
  wrong; `pycc_artifact_layout::anchor_target_root_for_build_script`
  reports that, and the build script emits a `cargo::warning` naming both
  paths and the three ways out. An absolute root — including the
  `<manifest_dir>/target` fallback — is never reported as divergent.

  **(vi) Two writers can target one path.** On `cargo build --workspace`
  Cargo itself uplifts `<root>/<profile>/libpycc_rt.a` while the build
  script installs the same file. The sharper case is cross-process:
  `cargo llvm-cov` holding the lock on `<root>/llvm-cov-target` while a
  concurrent plain `cargo build` is mid-uplift of `<root>/debug`. It is
  the same crate at the same version, so the bytes are equivalent, and the
  install writes to a uniquely named temporary **inside the destination
  directory** and renames it into place, so no reader ever sees a torn
  archive. What can still be perturbed is the other Cargo's mtime
  fingerprinting, which costs at most one rebuild.

  One rule follows and is not optional: the install **renames, then
  backdates the file it just renamed** — never `set_times` on a
  pre-existing destination. After a Cargo uplift that destination is a
  hardlink into Cargo's own `deps/` directory, and backdating through it
  would corrupt Cargo's fingerprint for a file Cargo believes it owns.

  **(vii) `cargo check`, clippy, and rust-analyzer pay for this.** Build
  scripts run under `cargo check`, so every check cycle is a potential
  nested build. Measured on this change: a warm
  `cargo check -p pycc_codegen` is **0.03 s** and does not rerun the
  script at all; the two clean-root harnesses together — each a cold
  nested build of both profiles — take **10.9 s**. Two rules keep the
  steady state at zero work. The archive's mtime is stamped to a constant
  far-past timestamp (2020-01-01T00:00:00Z) so that the installed file,
  which is itself a `rerun-if-changed` input, never looks newer than the
  script's own output and never re-arms the script. And the install is
  idempotent — when the destination already holds the same bytes, nothing
  is written — so the one rerun that a `pycc_rt` recompile does cause
  costs a warm nested cargo and no write, and damps immediately.

  **(viii) Build-script stdout must be byte-identical across runs.** Cargo
  hashes a build script's emitted directives into its dependents'
  fingerprints, so any variation would invalidate the whole downstream
  graph on every build. The script therefore emits its full directive set
  in a fixed order before any early return, and never prints the unique
  temporary filename, a timing, or a pid. The `cargo::warning` text is a
  function of the paths alone and so is deterministic too.

  **(ix) `pycc_codegen` is not packageable, and now visibly so.** A
  `build.rs` that reads `../pycc_rt` puts a path outside the package
  directory on the build path. This is pre-existing rather than new —
  `docs/DISTRIBUTION.md` already records that internal path-only
  dependencies block crates.io publication, enforced by
  `scripts/check_package_identity.rb` — but it is written down here so
  that the future publication work ([#210](https://github.com/rotnov/pycc/issues/210))
  inherits the constraint explicitly instead of rediscovering it.

  **(x) This supersedes the position recorded in `crates/pycc_rt/src/lib.rs`.**
  That header stated that fixing the build-order sharp edge for real
  "needs either a separate target-dir for the nested build or embedding
  `pycc_rt` into the `pycc` binary directly — both bigger changes than
  this sharp edge currently justifies." The first of those two is exactly
  what this decision does, and it turned out to be small. The header is
  rewritten in the same change.

- Alternatives:

  - **Artifact dependencies (`-Z bindeps`).** The right long-term answer
    and the one Cargo is building. Rejected because it is nightly-only and
    this project pins `rust-version = "1.97.1"`; revisit when it
    stabilizes.
  - **Embed the archive in the `pycc` binary** (`include_bytes!` the
    staticlib, write it to a temporary directory at link time). Removes
    the shared-destination question entirely, but moves a
    multi-megabyte artifact into the binary, still needs a build script to
    produce it, and makes `--target` cross-compilation strictly worse.
  - **A nested build at the outer target directory.** Measured to
    deadlock on Cargo's build-directory lock. Not a tradeoff, a hang.
  - **`env_clear()` plus an allowlist** for the nested environment. See
    (iii): correct on macOS, unverifiable on Windows MSVC.
  - **Build only the profile under construction.** Cheaper, and leaves
    `pycc build --release` broken after a plain `cargo build` — the exact
    class of breakage this exists to remove.
  - **Leave it to CI** by keeping the explicit `cargo build -p pycc_rt`
    pre-build step. That is the status quo #20 rejects: it makes a clean
    checkout fail its own tests for a contributor who has not read a
    workflow file.

- Consequences:

  - A clean checkout builds and tests with no manual pre-build step. The
    `no pycc_rt build found … Run \`cargo build -p pycc_rt\` first.`
    diagnostic is retained verbatim, because the cases that still reach it
    — a cross-compilation target, or a target directory redirected by an
    input `pycc` cannot observe — are exactly the cases where running that
    command by hand is the fix.
  - Cross-compiling `pycc_rt` for a `--target` triple the workspace was
    not itself built for is still explicit (`rustup target add` plus
    `cargo build --target …`). Making it automatic would mean
    reimplementing rustup's target management inside a build script.
  - `--target-dir` on the command line, and `build.target-dir` in a
    `.cargo/config.toml`, now redirect a **write** and not merely a read:
    Cargo exports no `CARGO_TARGET_DIR` for either, so the resolver falls
    back to `<workspace>/target` and the build script creates and writes
    there while Cargo builds elsewhere. This is self-consistent with
    D-183's read-side behavior but it is new, and it sits beside the
    relative-`CARGO_TARGET_DIR` asymmetry in (v).
    [#639](https://github.com/rotnov/pycc/issues/639) tracks whether
    either input should be honored at all.
  - A user-defined Cargo profile (`[profile.custom]`) resolves to
    `<root>/custom/`, a directory this build script installs nothing into.
    Both host profiles are still produced, so nothing regresses relative
    to the previous manual step, but the archive is not where such a build
    would look.
  - `cargo build --target <triple>` places artifacts under
    `<root>/<triple>/<profile>/` whenever `--target` is passed at all, even
    when the triple is the host's own default. A build script sees only the
    resolved `TARGET`, never whether the flag was given, so this script
    keys on `TARGET != HOST` and installs at the unqualified
    `<root>/<profile>/` in that same-triple case. Nothing mislinks: the
    existing `no pycc_rt build found (expected ...)` diagnostic fires and
    names the directory it searched, exactly as it did before this change.
    It is the same shape of gap as the custom-profile case above, and it is
    recorded for the same reason.
  - Windows is the untested leg. Every measurement behind this entry is
    macOS 15 on arm64. The `.lib` naming, MSVC environment survival under
    the namespace scrub, and rename-over-an-existing-file semantics are
    reasoned about rather than observed, and CI is the first place they
    run.
