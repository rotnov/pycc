//! Issue #146 Part 1: public-CLI coverage for heap-bigint refcounting at
//! named-storage and loop-induction sites.
//!
//! D-058 originally leaked every `BigIntObj` forever. #147 (D-179) widened
//! that leak to one object per `range` iteration, which is what makes it
//! observable as unbounded process growth rather than a bounded overflow-path
//! concession. Part 1 gives `BigIntObj` a refcount and releases it at the two
//! site families where a *named* storage location stops referring to a word:
//! ordinary assignment targets and `for`/comprehension induction variables.
//!
//! Two families live here:
//!
//! * **Value correctness under release.** Every shape below would print the
//!   wrong value, or crash, if a release fired while a live name still
//!   referred to the word. The aliasing shapes are the interesting ones: a
//!   `range` whose bound is a *named local* makes the loop's `start_v`,
//!   `stop_v`, and the visible induction target all alias one heap object,
//!   so an unbalanced release there is a use-after-free rather than a leak.
//! * **Peak-RSS ratio.** A missed *release* leaves the leak in place and is
//!   invisible to every value assertion above. The ratio test at the bottom
//!   of this file is the one that fails loudly for it.

use std::io::Write;
use std::process::Command;

/// `2^62`, the smallest magnitude that does not round-trip through D-061's
/// tagged 63-bit encoding and therefore always allocates a `BigIntObj`.
const PROMOTED: &str = "4611686018427387904";

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

fn write_case(case: &str, source: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pycc_issue146_{case}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("case.py");
    std::fs::File::create(&src)
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    src
}

/// Runs `source` through `pycc run` and asserts it exits `0` with exactly
/// `expected` on stdout.
fn assert_runs_and_prints(case: &str, source: &str, expected: &str) {
    let src = write_case(case, source);
    let run = Command::new(pycc_bin())
        .args(["run", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        run.status.code(),
        Some(0),
        "{case} should run to completion, got {:?}: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "{case} printed the wrong output"
    );
}

// ---------------------------------------------------------------------------
// Aliasing: a `range` bound that is a named local.
//
// Every pre-existing bigint-`range` artifact (`tests/issue_147_bigint_range.
// rs`, `tests/fixtures/bigint_range.py`) uses *literal* bounds. A literal
// bound is materialized fresh at rc=1 per evaluation and is referred to by no
// name, so its birth reference silently absorbs one extra release -- a
// `start_v` double-release would still pass every one of those tests. These
// shapes are the ones that do not absorb it: the bound outlives the loop
// under a name, so it must still be printable afterwards.
// ---------------------------------------------------------------------------

#[test]
fn a_named_bigint_start_bound_survives_the_loop_that_consumes_it() {
    // On iteration 1 the induction phi's incoming value *is* `start_v`, which
    // is *also* the word held by `b`'s storage slot. Three names for one
    // object; exactly one of them (`b`'s slot) still owns a reference when
    // the loop ends.
    assert_runs_and_prints(
        "named_start_bound",
        &format!("b: int = {PROMOTED}\nfor i in range(b, 4611686018427387907):\n    print(i)\nprint(b)\n"),
        "4611686018427387904\n4611686018427387905\n4611686018427387906\n4611686018427387904\n",
    );
}

#[test]
fn a_named_bigint_bound_used_for_both_start_and_stop_survives_an_empty_loop() {
    // `range(b, b)` is empty, so the loop body never runs and the induction
    // phi is only ever `start_v`. Both normalized operands alias `b`'s single
    // heap object, so the preheader retains it twice and `after_bb` must
    // release it exactly twice -- one release too many frees a word `b` still
    // names, and the trailing `print(b)` reads freed memory.
    assert_runs_and_prints(
        "aliased_empty_range",
        &format!("b: int = {PROMOTED}\nfor i in range(b, b):\n    print(i)\nprint(b)\n"),
        "4611686018427387904\n",
    );
}

#[test]
fn a_named_bigint_bound_used_for_start_stop_and_step_survives() {
    // The third operand joins the alias set. `step` must be non-zero, so this
    // uses `range(b, b, b)`: an empty loop again, now with three normalized
    // operands all naming the same object.
    assert_runs_and_prints(
        "aliased_three_operands",
        &format!("b: int = {PROMOTED}\nfor i in range(b, b, b):\n    print(i)\nprint(b)\n"),
        "4611686018427387904\n",
    );
}

#[test]
fn a_bigint_loop_variable_reassigned_to_a_smallint_inside_the_body_still_iterates() {
    // Overwriting the visible target inside the body releases the bigint the
    // bind just retained. The induction phi keeps its own reference, so the
    // loop must keep advancing from the right value regardless.
    assert_runs_and_prints(
        "reassign_bound_target",
        &format!("for i in range({PROMOTED}, 4611686018427387907):\n    print(i)\n    i = 5\n    print(i)\n"),
        "4611686018427387904\n5\n4611686018427387905\n5\n4611686018427387906\n5\n",
    );
}

// ---------------------------------------------------------------------------
// Named storage: assignment, globals, parameters, returns, attributes.
// ---------------------------------------------------------------------------

#[test]
fn repeatedly_overwriting_a_named_bigint_local_keeps_the_live_value_intact() {
    assert_runs_and_prints(
        "overwrite_named_local",
        &format!("x: int = {PROMOTED}\nx = x + 1\nx = x + 1\nprint(x)\n"),
        "4611686018427387906\n",
    );
}

#[test]
fn aliasing_a_named_bigint_local_keeps_both_names_readable() {
    // `y = x` is the duplicate-reference shape D-060 already handles for
    // `str`: the retain on the source is what keeps `x` alive once `x` is
    // itself overwritten.
    assert_runs_and_prints(
        "alias_named_local",
        &format!("x: int = {PROMOTED}\ny: int = x\nx = 0\nprint(y)\n"),
        "4611686018427387904\n",
    );
}

#[test]
fn a_bigint_module_global_survives_being_read_from_a_function() {
    assert_runs_and_prints(
        "global_read_from_function",
        &format!(
            "g: int = {PROMOTED}\n\ndef read() -> int:\n    return g\n\nprint(read())\nprint(g)\n"
        ),
        "4611686018427387904\n4611686018427387904\n",
    );
}

#[test]
fn a_bigint_passed_as_an_argument_survives_the_call() {
    assert_runs_and_prints(
        "argument_survives_call",
        &format!(
            "def twice(v: int) -> int:\n    return v + v\n\nx: int = {PROMOTED}\nprint(twice(x))\nprint(x)\n"
        ),
        "9223372036854775808\n4611686018427387904\n",
    );
}

#[test]
fn a_bigint_returned_from_a_function_survives_the_return() {
    assert_runs_and_prints(
        "return_survives",
        &format!("def make() -> int:\n    v: int = {PROMOTED}\n    return v\n\nprint(make())\n"),
        "4611686018427387904\n",
    );
}

#[test]
fn a_bigint_stored_in_an_instance_attribute_survives_a_later_overwrite() {
    assert_runs_and_prints(
        "attribute_overwrite",
        &format!(
            "from dataclasses import dataclass\n\n@dataclass\nclass Box:\n    v: int\n\nb = Box({PROMOTED})\nprint(b.v)\nb.v = b.v + 1\nprint(b.v)\n"
        ),
        "4611686018427387904\n4611686018427387905\n",
    );
}

#[test]
fn a_bool_assigned_into_a_bigint_valued_int_slot_releases_the_old_word() {
    // `value.ty()` here is `Ty::Bool`, not `Ty::Int`: the release has to be
    // gated on the *slot's* declared type or this store silently skips it.
    // `coerce_scalar_to_type` turns the `i8` into D-141's encoded word `6`.
    assert_runs_and_prints(
        "bool_into_int_slot",
        &format!("x: int = {PROMOTED}\nx = True\nprint(x)\n"),
        // D-141 keeps bool identity through the int-encoded word, so this
        // prints `True`, not `1`.
        "True\n",
    );
}

// ---------------------------------------------------------------------------
// Comprehensions: the same induction-variable ownership, three more emitters.
// ---------------------------------------------------------------------------

#[test]
fn a_list_comprehension_over_a_named_bigint_bound_leaves_the_bound_intact() {
    assert_runs_and_prints(
        "list_comp_named_bound",
        &format!("b: int = {PROMOTED}\nxs = [0 for i in range(b, 4611686018427387906)]\nprint(len(xs))\nprint(b)\n"),
        "2\n4611686018427387904\n",
    );
}

#[test]
fn a_set_comprehension_over_a_named_bigint_bound_leaves_the_bound_intact() {
    assert_runs_and_prints(
        "set_comp_named_bound",
        &format!("b: int = {PROMOTED}\nxs = {{0 for i in range(b, 4611686018427387906)}}\nprint(len(xs))\nprint(b)\n"),
        "1\n4611686018427387904\n",
    );
}

#[test]
fn a_dict_comprehension_over_a_named_bigint_bound_leaves_the_bound_intact() {
    assert_runs_and_prints(
        "dict_comp_named_bound",
        &format!("b: int = {PROMOTED}\nxs = {{\"k\": 0 for i in range(b, 4611686018427387906)}}\nprint(len(xs))\nprint(b)\n"),
        "1\n4611686018427387904\n",
    );
}

// ---------------------------------------------------------------------------
// Peak-RSS ratio: the test that actually fails for a *missed release*.
//
// Deliberately a ratio and never an absolute bound: `rusage::ru_maxrss` is
// bytes on macOS/BSD and kilobytes on Linux, and the baseline footprint of a
// pycc binary differs per platform and per libc. A leak of one `BigIntObj`
// per iteration is linear in the trip count, so doubling the iterations
// doubles the peak. A fixed program's peak is flat.
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod peak_rss {
    use super::{pycc_bin, write_case, PROMOTED};
    use std::process::Command;

    /// Spawns `command`, waits for it via `wait4`, and returns the child's
    /// `ru_maxrss`. Follows `tests/nbody_bench.rs::time_command`'s reaping
    /// pattern (duplicated rather than shared -- this repository's
    /// integration tests each carry their own helpers, see that file's
    /// `oracle_python_bin` doc comment), but reads the resident-set high
    /// water mark instead of the CPU times.
    #[allow(clippy::zombie_processes)]
    fn peak_rss(mut command: Command) -> libc::c_long {
        let child = command.spawn().expect("command must spawn");
        let pid = child.id() as libc::pid_t;
        let mut status = 0;
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        let waited = loop {
            // SAFETY: `pid` identifies the live child spawned immediately
            // above; both output pointers are valid for writes for the
            // duration of the call, and a successful `wait4` initializes the
            // complete `rusage` value.
            let result = unsafe { libc::wait4(pid, &mut status, 0, usage.as_mut_ptr()) };
            if result != -1 {
                break result;
            }
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::Interrupted,
                "wait failed for {command:?}: {error}"
            );
        };
        assert_eq!(waited, pid, "wait4 reaped the wrong child for {command:?}");
        // SAFETY: the successful `wait4` above initialized `usage`.
        let usage = unsafe { usage.assume_init() };
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "command failed: {command:?}"
        );
        // `ru_maxrss` is `i64` on macOS/BSD and `i64` on 64-bit Linux, but
        // the field is the *peak resident set*, in bytes on macOS/BSD and in
        // kilobytes on Linux -- which is why every assertion below is a
        // ratio of two readings from the same platform, never an absolute.
        usage.ru_maxrss
    }

    /// Builds `source` with `pycc build` and returns the produced binary's
    /// own peak RSS -- never `pycc run`'s, whose footprint is dominated by
    /// LLVM and would swamp the signal.
    fn built_program_peak_rss(case: &str, source: &str) -> libc::c_long {
        let src = write_case(case, source);
        let bin = std::env::temp_dir().join(format!(
            "pycc_issue146_rss_{case}_{}",
            std::process::id()
        ));
        let build = Command::new(pycc_bin())
            .arg("build")
            .arg(&src)
            .arg("-o")
            .arg(&bin)
            .status()
            .expect("pycc build must spawn");
        assert!(build.success(), "pycc build of {case} failed");
        peak_rss(Command::new(&bin))
    }

    /// The #146 repro, reduced to Part 1's own scope: each iteration binds a
    /// freshly allocated `BigIntObj` to the same named local, dropping the
    /// previous one on the floor.
    ///
    /// Deliberately `x = x + 1` rather than `x = <bigint literal> + i`: a
    /// bigint *literal* is materialized per evaluation
    /// (`int_const::emit_int_constant`), so the literal form leaks one extra
    /// unbound temporary per iteration on top of the named-storage leak this
    /// test is about. That temporary is a discarded arithmetic value with no
    /// name, which is Part 2 (#625) and explicitly out of Part 1's scope --
    /// measuring it here would make this gate unpassable for reasons Part 1
    /// cannot fix.
    fn repro(iterations: u32) -> String {
        format!("x: int = {PROMOTED}\nfor i in range({iterations}):\n    x = x + 1\nprint(x)\n")
    }

    #[test]
    fn overwriting_a_named_bigint_local_does_not_grow_with_the_iteration_count() {
        let single = built_program_peak_rss("rss_1x", &repro(500_000));
        let double = built_program_peak_rss("rss_2x", &repro(1_000_000));
        let ratio = double as f64 / single as f64;
        assert!(
            ratio < 1.35,
            "peak RSS must not scale with the loop trip count: \
             500k iterations={single} 1M iterations={double} ratio={ratio:.4} \
             (a per-iteration `BigIntObj` leak reads as ratio ~2.0)"
        );
    }

    /// The same measurement for the bigint-domain `range` #147 introduced,
    /// where the leaked object is the induction variable itself rather than
    /// an assignment target.
    #[test]
    fn a_bigint_domain_range_loop_does_not_grow_with_the_iteration_count() {
        let start: u128 = 4_611_686_018_427_387_904;
        let single = built_program_peak_rss(
            "rss_range_1x",
            &format!(
                "for i in range({start}, {}):\n    pass\nprint(0)\n",
                start + 500_000
            ),
        );
        let double = built_program_peak_rss(
            "rss_range_2x",
            &format!(
                "for i in range({start}, {}):\n    pass\nprint(0)\n",
                start + 1_000_000
            ),
        );
        let ratio = double as f64 / single as f64;
        assert!(
            ratio < 1.35,
            "peak RSS must not scale with the loop trip count: \
             500k iterations={single} 1M iterations={double} ratio={ratio:.4} \
             (a per-iteration `BigIntObj` leak reads as ratio ~2.0)"
        );
    }
}
