//! # Build order: this crate's staticlib is not a normal Cargo dependency
//!
//! This crate's real consumer is not Rust code but pycc-generated object
//! files, which reference `pycc_rt_print_i64` by symbol name and are
//! linked against this crate's `staticlib` output (`libpycc_rt.a` on
//! Unix-like targets, `pycc_rt.lib` on `-msvc` targets -- see D-028) via a
//! linker-driver invocation (`cc`, or on Windows the bundled `clang` --
//! see D-028) -- in `pycc_codegen`'s own tests (`link_object_with_runtime`)
//! and in `pycc`'s real `build`/`run` (`src/main.rs`). Nothing in Cargo's
//! normal dependency graph expresses that relationship, so this crate's
//! staticlib output only exists once this crate has actually been built
//! explicitly (or as part of a workspace-wide build) -- unlike this
//! source file, which is of course always there.
//!
//! **Practical consequence:** commands scoped to a single other crate --
//! `cargo test -p pycc_codegen`, `cargo run --bin pycc -- build ...` run in
//! isolation -- need this crate built first: `cargo build -p pycc_rt`.
//! `cargo build --workspace` / `cargo test --workspace` (what CI always
//! runs) builds every workspace member including this one, so the ordering
//! issue never surfaces there.
//!
//! A `build.rs` in the consuming crates that shells out to `cargo build -p
//! pycc_rt` was tried and reverted: pointed at the same `target-dir` as the
//! outer build, it deadlocks on Cargo's own build lock (the outer build
//! holds it for the whole build-script execution; the nested `cargo build`
//! blocks forever waiting for it). Fixing this for real needs either a
//! separate target-dir for the nested build or embedding `pycc_rt` into the
//! `pycc` binary directly instead of linking a sibling staticlib -- both
//! bigger changes than this sharp edge currently justifies.

fn format_i64_line(value: i64) -> String {
    format!("{value}\n")
}

/// See D-052: every `Ty::Int` value is one LLVM `i64`. Its low bit is the
/// discriminant -- `1` means the high 63 bits (arithmetic-shift-recovered)
/// are the real value; `0` means the full 64 bits are a heap `BigInt`
/// pointer (Task 9). This module never constructs the `0` case yet.
const TAG_BIT: i64 = 1;

fn tag_smallint(value: i64) -> i64 {
    (value << 1) | TAG_BIT
}

fn untag_smallint(tagged: i64) -> i64 {
    tagged >> 1 // arithmetic (sign-extending) shift for `i64`
}

fn is_smallint(tagged: i64) -> bool {
    tagged & TAG_BIT == TAG_BIT
}

/// `None` when `value` needs the full 64 bits (including sign) to
/// represent -- i.e. tagging then untagging would not round-trip.
fn fits_smallint(value: i64) -> Option<i64> {
    let tagged = tag_smallint(value);
    (untag_smallint(tagged) == value).then_some(tagged)
}

fn require_smallint(tagged: i64, context: &str) {
    if !is_smallint(tagged) {
        panic!("pycc_rt: {context} a bigint-valued `int` is not supported yet");
    }
}

// --- Implementation note / deviation from the task brief -------------
//
// The brief's own doc comment (kept, below, as the historical record of
// the *intended* design) claims that a panic raised by calling one of
// these `pycc_rt_int_*` functions directly from this crate's own Rust
// test code "is an ordinary, same-binary unwind the test harness
// catches -- no FFI boundary is crossed during the test itself." That
// claim does not hold on this toolchain (rustc 1.97.1): whether a panic
// unwinding past a function's own boundary gets caught and turned into
// an abort is a property of *that function's own* declared ABI, decided
// at the point the function itself is compiled -- not of who happens to
// call it. A plain `extern "C" fn` (not `extern "C-unwind"`) always gets
// this abort-on-unwind landing pad around its own body, so calling it
// directly from ordinary same-crate Rust test code panics into a
// `SIGABRT` (confirmed empirically: the `#[should_panic]` tests below
// aborted the whole test binary instead of being caught, before this
// split existed).
//
// The *intent* behind choosing plain `extern "C"` is still correct and
// worth keeping: pycc-generated LLVM IR has no personality routine/unwind
// tables, so a real unwind escaping into it would be genuinely unsafe --
// converting that into a deterministic abort right at this boundary is
// the right call for the actual compiled-program scenario. Only the
// "therefore it's directly unit-testable with `#[should_panic]`" half of
// the claim was wrong.
//
// Fix: split each `pycc_rt_int_*` symbol into a private, ordinary-Rust-ABI
// function holding the real logic (freely panics, unwinds normally, so
// `#[should_panic]` can catch it when called directly) and a thin
// `#[unsafe(no_mangle)] pub extern "C" fn` wrapper of the exact name/
// signature the brief specifies, which every later task's codegen still
// calls unchanged. Tests exercising a panicking path call the private
// function directly; tests only checking a successful return value keep
// calling the public wrapper (also exercising the wrapper's own line, no
// unwind ever crosses its boundary on those paths).
fn int_add(a: i64, b: i64) -> i64 {
    require_smallint(a, "adding");
    require_smallint(b, "adding");
    untag_smallint(a)
        .checked_add(untag_smallint(b))
        .and_then(fits_smallint)
        .unwrap_or_else(|| panic!("pycc_rt: integer overflow (bigint promotion is not implemented yet)"))
}

/// # Safety (panic-across-FFI note, applies to every `pycc_rt_int_*`
/// function below)
/// These are plain `extern "C" fn`s, not `extern "C-unwind"`. Since Rust
/// 1.71, a panic that would otherwise unwind past an ordinary
/// `extern "C"` function's boundary is caught at that boundary and turned
/// into a process abort instead of continuing to unwind into a foreign
/// (non-Rust, no unwind tables) caller -- which is exactly what happens
/// here when pycc-generated LLVM code calls one of these and it panics.
/// This is a real, stable Rust guarantee (not assumed UB-avoidance) --
/// see the implementation-note comment above this function for why that
/// same guarantee makes calling *this* wrapper directly unsuitable for
/// `#[should_panic]` testing (the private `int_add` etc. functions above
/// are used for that instead).
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_add(a: i64, b: i64) -> i64 {
    int_add(a, b)
}

fn int_sub(a: i64, b: i64) -> i64 {
    require_smallint(a, "subtracting");
    require_smallint(b, "subtracting");
    untag_smallint(a)
        .checked_sub(untag_smallint(b))
        .and_then(fits_smallint)
        .unwrap_or_else(|| panic!("pycc_rt: integer overflow (bigint promotion is not implemented yet)"))
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_sub(a: i64, b: i64) -> i64 {
    int_sub(a, b)
}

fn int_mul(a: i64, b: i64) -> i64 {
    require_smallint(a, "multiplying");
    require_smallint(b, "multiplying");
    untag_smallint(a)
        .checked_mul(untag_smallint(b))
        .and_then(fits_smallint)
        .unwrap_or_else(|| panic!("pycc_rt: integer overflow (bigint promotion is not implemented yet)"))
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_mul(a: i64, b: i64) -> i64 {
    int_mul(a, b)
}

fn int_floordiv(a: i64, b: i64) -> i64 {
    require_smallint(a, "dividing");
    require_smallint(b, "dividing");
    let (a, b) = (untag_smallint(a), untag_smallint(b));
    if b == 0 {
        panic!("pycc_rt: integer division by zero");
    }
    // Deviation from the task brief: the brief's own code guarded here
    // against the classic hardware trap on a raw `i64::MIN / -1` (the
    // mathematical quotient `2^63` doesn't fit `i64`, and Rust's checked
    // `/`/`%` themselves panic/trap on that exact pair). That guard is
    // unreachable dead code under D-052's fixed tagged representation:
    // `a`/`b` here are already `untag_smallint`-ed from a valid tagged
    // `i64` argument, and for *every* `i64` value `t`, `t >> 1` (what
    // `untag_smallint` computes) lands in `[i64::MIN >> 1, i64::MAX >>
    // 1]` -- strictly inside `i64`'s own range and excluding `i64::MIN`
    // itself (verified: `is_smallint`/`require_smallint` only check the
    // tag *bit*, and every odd `i64` round-trips through
    // `tag_smallint`/`untag_smallint` exactly, so this bound holds for
    // literally every value that can reach this function, not just
    // "typical" callers). `cargo llvm-cov`'s region coverage confirmed
    // this empirically: the removed branch's body never executed under
    // any test, including one written specifically to try to hit it.
    // The `fits_smallint` check below still catches the *actual*
    // reachable overflow case: floor-dividing the minimum taggable value
    // by `-1` negates it, producing exactly one more than the maximum
    // taggable magnitude (see
    // `pycc_rt_int_floordiv_panics_when_negating_the_minimum_taggable_value_overflows`).
    let q = a / b;
    let r = a % b;
    let floored = if r != 0 && (r < 0) != (b < 0) { q - 1 } else { q };
    fits_smallint(floored)
        .unwrap_or_else(|| panic!("pycc_rt: integer overflow (bigint promotion is not implemented yet)"))
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_floordiv(a: i64, b: i64) -> i64 {
    int_floordiv(a, b)
}

fn int_floormod(a: i64, b: i64) -> i64 {
    require_smallint(a, "computing the modulo of");
    require_smallint(b, "computing the modulo of");
    let (a, b) = (untag_smallint(a), untag_smallint(b));
    if b == 0 {
        panic!("pycc_rt: integer modulo by zero");
    }
    // Deviation from the task brief: the brief's own code special-cased
    // `a == i64::MIN && b == -1` here (mirroring `int_floordiv`'s
    // original guard) to sidestep the same raw `%` hardware trap. Under
    // D-052's fixed tagged representation this is unreachable for the
    // same reason `int_floordiv`'s removed guard was (see its comment):
    // an already-tagged operand's untagged form can never equal
    // `i64::MIN`. Floor-mod's *result* can't overflow the taggable range
    // either -- unlike floor-division, which the comment on
    // `int_floordiv` explains can: floor-mod's result always satisfies
    // `|result| < |b|`, and every already-tagged `b` satisfies `|b| <=
    // 2^62` (D-052's 63-bit range), so `floored` always re-fits and the
    // `fits_smallint` round-trip check the brief had here (like
    // `int_floordiv`'s) is provably always-`Some` -- confirmed by
    // `cargo llvm-cov`: its `None` arm never executed under any test.
    // `tag_smallint` alone is therefore correct and simpler.
    let r = a % b;
    let floored = if r != 0 && (r < 0) != (b < 0) { r + b } else { r };
    tag_smallint(floored)
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_floormod(a: i64, b: i64) -> i64 {
    int_floormod(a, b)
}

fn int_pow(base: i64, exp: i64) -> i64 {
    require_smallint(base, "exponentiating");
    require_smallint(exp, "exponentiating");
    let mut exp = untag_smallint(exp);
    if exp < 0 {
        panic!(
            "pycc_rt: negative exponent for `int ** int` is not supported \
             (the real result would need to be `float`, matching CPython's \
             own `int ** int` rule -- a pre-existing pycc_types simplification, \
             not a new PR-5 gap: pycc_types::numeric_result_type always types \
             `**` as `int`-returning)"
        );
    }
    let mut result = tag_smallint(1);
    let mut base = base;
    while exp > 0 {
        if exp & 1 == 1 {
            result = int_mul(result, base);
        }
        exp >>= 1;
        if exp > 0 {
            base = int_mul(base, base);
        }
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_pow(base: i64, exp: i64) -> i64 {
    int_pow(base, exp)
}

fn int_cmp(a: i64, b: i64) -> i32 {
    require_smallint(a, "comparing");
    require_smallint(b, "comparing");
    match untag_smallint(a).cmp(&untag_smallint(b)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_cmp(a: i64, b: i64) -> i32 {
    int_cmp(a, b)
}

fn int_print(tagged: i64) {
    require_smallint(tagged, "printing");
    pycc_rt_print_i64(untag_smallint(tagged));
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_print(tagged: i64) {
    int_print(tagged);
}

/// # Safety
/// Called only from pycc-generated code with a plain i64 argument; no
/// pointers involved, so there is nothing for the caller to uphold beyond
/// standard `extern "C"` calling-convention correctness.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_print_i64(value: i64) {
    print!("{}", format_i64_line(value));
}

/// `int`'s truthiness for `if`/`while` conditions (Task 4). Never panics --
/// unlike every `pycc_rt_int_*` arithmetic/comparison function above, this
/// one has no failure mode to guard against, so (per this crate's
/// established convention -- see the implementation note above `int_add`)
/// it does not need the private-logic/public-wrapper split: nothing here
/// ever unwinds, so there's no abort-vs-catch distinction for a caller to
/// trip over.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_truthy(tagged: i64) -> i8 {
    // A value tagged as a heap `BigInt` (Task 9) is, by construction,
    // only ever created because it *didn't* fit the smallint range --
    // which excludes zero -- so it's always truthy without needing to
    // inspect it further.
    if !is_smallint(tagged) {
        return 1;
    }
    i8::from(untag_smallint(tagged) != 0)
}

// --- Implementation note / deviation from the task brief -------------
//
// The brief's own Step 2 code makes `pycc_rt_range_continue` a single
// plain `extern "C" fn`, with a Step 1 `#[should_panic]` test calling that
// same public wrapper directly for the zero-step case. Per this crate's
// own established convention (see the implementation-note comment above
// `int_add`, discovered empirically during Task 3): a panic that unwinds
// past a plain `extern "C" fn`'s own boundary is caught right there and
// turned into a process abort, regardless of who calls it -- including
// this crate's own same-binary Rust tests. `pycc_rt_range_continue` *can*
// panic (`require_smallint`'s bigint-rejection path, and the zero-step
// case below), so it needs the same split every other panicking
// `pycc_rt_int_*` function already gets: a private, ordinary-Rust-ABI
// `range_continue` holding the real logic (freely panics, unwinds
// normally, `#[should_panic]`-testable), and a thin `pub extern "C"`
// wrapper of the exact brief-specified name/signature for pycc-generated
// code to call. The zero-step test below calls `range_continue` directly,
// not the public wrapper, for the same reason every other
// `#[should_panic]` test in this file does.
fn range_continue(i: i64, stop: i64, step: i64) -> i8 {
    require_smallint(i, "iterating");
    require_smallint(stop, "iterating");
    require_smallint(step, "iterating");
    let (i, stop, step) = (untag_smallint(i), untag_smallint(stop), untag_smallint(step));
    match step.cmp(&0) {
        std::cmp::Ordering::Greater => i8::from(i < stop),
        std::cmp::Ordering::Less => i8::from(i > stop),
        std::cmp::Ordering::Equal => panic!("pycc_rt: range() arg 3 must not be zero"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_range_continue(i: i64, stop: i64, step: i64) -> i8 {
    range_continue(i, stop, step)
}

/// Converts a tagged `int` (D-052) to its `f64` value -- the `int` half of
/// Python's `int`/`float` arithmetic promotion (Task 6). Can panic (via
/// `require_smallint`'s bigint-rejection path), so -- per this crate's
/// established convention, see the implementation note above `int_add` --
/// this is split into this private, ordinary-Rust-ABI function (freely
/// panics, unwinds normally) and a thin `pub extern "C"` wrapper below.
/// Deviation from the task brief: the brief's own Step 2 code made this a
/// single plain `extern "C" fn`; that would abort (rather than unwind)
/// if this function's own panic path were ever exercised directly from
/// this crate's same-binary Rust tests, exactly the hazard the
/// `int_add`/`range_continue` split comments already document -- this
/// function is no exception just because its own tests don't currently
/// hit that path directly.
fn int_to_float(tagged: i64) -> f64 {
    require_smallint(tagged, "converting");
    untag_smallint(tagged) as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_to_float(tagged: i64) -> f64 {
    int_to_float(tagged)
}

/// Python's `//` on `float`: floors toward negative infinity, not
/// truncation toward zero (matches `int_floordiv`'s own semantics, just
/// without the tagging/overflow bookkeeping a `float` doesn't need).
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_float_floordiv(a: f64, b: f64) -> f64 {
    (a / b).floor()
}

/// Python's `%` on `float`: result takes the divisor's sign (matches
/// `int_floormod`'s own semantics).
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_float_floormod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && (r < 0.0) != (b < 0.0) { r + b } else { r }
}

/// Python's `**` on `float`: unlike `int_pow`, a negative exponent is
/// perfectly ordinary here (`2.0 ** -1 == 0.5`) -- `f64::powf` already
/// implements this correctly, no special-casing needed.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_float_pow(a: f64, b: f64) -> f64 {
    a.powf(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_i64_matches_cpython_format() {
        // Confirmed against `python3.14 -c "print(42)"` / `print(-7)`:
        // exactly the digits, then a single trailing newline, nothing else.
        assert_eq!(format_i64_line(42), "42\n");
        assert_eq!(format_i64_line(-7), "-7\n");
    }

    #[test]
    fn extern_c_entry_point_runs_for_positive_negative_and_zero() {
        // This crate's staticlib output is linked into pycc-compiled
        // binaries by a separate `cc` step (Task 8), which cargo-llvm-cov
        // never instruments -- so this is the only place
        // pycc_rt_print_i64 itself (not just its formatting helper) is
        // exercised for the D-014 gate. stdout is captured by the test
        // harness and only shown on failure.
        pycc_rt_print_i64(42);
        pycc_rt_print_i64(-7);
        pycc_rt_print_i64(0);
    }

    #[test]
    fn tagging_a_small_value_and_untagging_it_round_trips() {
        for n in [0i64, 1, -1, 42, -42, i64::MAX >> 1, -(i64::MAX >> 1)] {
            let tagged = tag_smallint(n);
            assert!(is_smallint(tagged), "expected {n} to be tagged as small");
            assert_eq!(untag_smallint(tagged), n);
        }
    }

    #[test]
    fn a_value_needing_the_full_64_bits_does_not_fit_the_tagged_range() {
        assert_eq!(fits_smallint(i64::MAX), None);
        assert_eq!(fits_smallint(i64::MIN), None);
    }

    #[test]
    fn pycc_rt_int_add_computes_the_correct_tagged_sum() {
        let a = tag_smallint(2);
        let b = tag_smallint(3);
        assert_eq!(untag_smallint(pycc_rt_int_add(a, b)), 5);
    }

    #[test]
    #[should_panic(expected = "integer overflow")]
    fn pycc_rt_int_add_panics_on_overflow_before_bigint_promotion_exists() {
        // Calls the private `int_add`, not the public `pycc_rt_int_add`
        // wrapper: see the implementation-note comment above `int_add`'s
        // definition -- a panic unwinding past a plain `extern "C" fn`'s
        // own boundary aborts the process regardless of caller, so only
        // the private, ordinary-Rust-ABI function is `#[should_panic]`-testable.
        int_add(tag_smallint(i64::MAX >> 1), tag_smallint(1));
    }

    #[test]
    fn pycc_rt_int_sub_computes_the_correct_tagged_difference() {
        let a = tag_smallint(5);
        let b = tag_smallint(3);
        assert_eq!(untag_smallint(pycc_rt_int_sub(a, b)), 2);
    }

    #[test]
    #[should_panic(expected = "integer overflow")]
    fn pycc_rt_int_sub_panics_on_overflow_before_bigint_promotion_exists() {
        // Not in the task brief's own Step 2 test list -- added because
        // `cargo llvm-cov`'s region coverage showed `int_sub`'s overflow
        // closure (a code region distinct from `int_add`'s, even though
        // both share the same message text) never executed under any
        // brief-supplied test, which the D-014 100% region-coverage gate
        // does not allow.
        int_sub(tag_smallint(i64::MIN >> 1), tag_smallint(1));
    }

    #[test]
    fn pycc_rt_int_mul_computes_the_correct_tagged_product() {
        let a = tag_smallint(6);
        let b = tag_smallint(7);
        assert_eq!(untag_smallint(pycc_rt_int_mul(a, b)), 42);
    }

    #[test]
    #[should_panic(expected = "integer overflow")]
    fn pycc_rt_int_mul_panics_on_overflow_before_bigint_promotion_exists() {
        // Same rationale as `pycc_rt_int_sub_panics_...` above: `int_mul`'s
        // own overflow closure was otherwise never exercised.
        int_mul(tag_smallint(i64::MAX >> 1), tag_smallint(2));
    }

    #[test]
    fn pycc_rt_int_floordiv_matches_python_floor_semantics() {
        // Python: -7 // 2 == -4 (floors toward negative infinity), not -3
        // (truncation toward zero, which is what a raw LLVM/Rust `/` gives).
        assert_eq!(untag_smallint(pycc_rt_int_floordiv(tag_smallint(-7), tag_smallint(2))), -4);
        assert_eq!(untag_smallint(pycc_rt_int_floordiv(tag_smallint(7), tag_smallint(2))), 3);
        assert_eq!(untag_smallint(pycc_rt_int_floordiv(tag_smallint(7), tag_smallint(-2))), -4);
    }

    #[test]
    fn pycc_rt_int_floormod_matches_python_floor_semantics() {
        // Python: -7 % 2 == 1 (result takes the divisor's sign), not -1.
        assert_eq!(untag_smallint(pycc_rt_int_floormod(tag_smallint(-7), tag_smallint(2))), 1);
        assert_eq!(untag_smallint(pycc_rt_int_floormod(tag_smallint(7), tag_smallint(2))), 1);
        assert_eq!(untag_smallint(pycc_rt_int_floormod(tag_smallint(7), tag_smallint(-2))), -1);
    }

    #[test]
    #[should_panic(expected = "division by zero")]
    fn pycc_rt_int_floordiv_by_zero_panics() {
        // See `pycc_rt_int_add_panics_...`'s comment above: private
        // function called directly so the panic is a catchable unwind.
        int_floordiv(tag_smallint(1), tag_smallint(0));
    }

    #[test]
    #[should_panic(expected = "modulo by zero")]
    fn pycc_rt_int_floormod_by_zero_panics() {
        int_floormod(tag_smallint(1), tag_smallint(0));
    }

    #[test]
    fn pycc_rt_int_floordiv_handles_the_minimum_taggable_value_divided_by_one() {
        // The task brief's own version of this test's comment claimed this
        // exercised the classic `i64::MIN / -1` hardware-trap pair -- it
        // does not (it divides by `1`, not `-1`); see `int_floordiv`'s own
        // comment for why that trap is actually unreachable here at all.
        // What this test does prove: the boundary value itself (the most
        // negative value that still tags) round-trips correctly through
        // floor-division by `1` without spuriously reporting overflow.
        let min = i64::MIN >> 1; // most negative value that still tags
        assert_eq!(untag_smallint(pycc_rt_int_floordiv(tag_smallint(min), tag_smallint(1))), min);
    }

    #[test]
    #[should_panic(expected = "integer overflow")]
    fn pycc_rt_int_floordiv_panics_when_negating_the_minimum_taggable_value_overflows() {
        // The actual reachable overflow case for `int_floordiv` (see its
        // comment): floor-dividing the minimum taggable value by `-1`
        // negates it, producing exactly one more than the maximum taggable
        // magnitude -- a real overflow of the *result*, distinct from (and
        // the reason the brief's original i64::MIN/-1 *input* guard turned
        // out to be dead code).
        let min = i64::MIN >> 1;
        int_floordiv(tag_smallint(min), tag_smallint(-1));
    }

    #[test]
    fn pycc_rt_int_pow_computes_the_correct_tagged_power() {
        assert_eq!(untag_smallint(pycc_rt_int_pow(tag_smallint(2), tag_smallint(10))), 1024);
        assert_eq!(untag_smallint(pycc_rt_int_pow(tag_smallint(5), tag_smallint(0))), 1);
    }

    #[test]
    #[should_panic(expected = "negative exponent")]
    fn pycc_rt_int_pow_with_a_negative_exponent_panics() {
        int_pow(tag_smallint(2), tag_smallint(-1));
    }

    #[test]
    fn pycc_rt_int_cmp_reports_less_equal_and_greater() {
        assert_eq!(pycc_rt_int_cmp(tag_smallint(1), tag_smallint(2)), -1);
        assert_eq!(pycc_rt_int_cmp(tag_smallint(2), tag_smallint(2)), 0);
        assert_eq!(pycc_rt_int_cmp(tag_smallint(3), tag_smallint(2)), 1);
    }

    #[test]
    #[should_panic(expected = "bigint-valued")]
    fn pycc_rt_int_cmp_on_a_bigint_tagged_operand_panics() {
        // Bit pattern `0` (even) is what D-052 reserves for a heap `BigInt`
        // pointer -- no real allocation needed to exercise this rejection.
        int_cmp(0, tag_smallint(1));
    }

    #[test]
    fn pycc_rt_int_print_prints_the_untagged_decimal_value() {
        // stdout is captured by the test harness; this exercises
        // `pycc_rt_int_print` itself (not just `pycc_rt_print_i64`) for the
        // D-014 gate, same rationale as this file's existing
        // `extern_c_entry_point_runs_for_positive_negative_and_zero` test.
        pycc_rt_int_print(tag_smallint(42));
        pycc_rt_int_print(tag_smallint(-7));
    }

    #[test]
    #[should_panic(expected = "bigint-valued")]
    fn pycc_rt_int_print_on_a_bigint_tagged_value_panics() {
        int_print(0);
    }

    #[test]
    fn pycc_rt_int_truthy_is_false_only_for_zero() {
        assert_eq!(pycc_rt_int_truthy(tag_smallint(0)), 0);
        assert_eq!(pycc_rt_int_truthy(tag_smallint(1)), 1);
        assert_eq!(pycc_rt_int_truthy(tag_smallint(-1)), 1);
    }

    #[test]
    fn pycc_rt_int_truthy_treats_any_bigint_tagged_value_as_truthy() {
        // Bit pattern `0` (even) is what D-052 reserves for a heap `BigInt`
        // pointer -- no real allocation needed to exercise this, same
        // precedent as `pycc_rt_int_cmp_on_a_bigint_tagged_operand_panics`
        // above. This is the only test exercising `pycc_rt_int_truthy`'s
        // early-return branch: the three assertions above only ever pass
        // smallint-tagged (odd) values.
        assert_eq!(pycc_rt_int_truthy(0), 1);
    }

    #[test]
    fn pycc_rt_range_continue_handles_positive_step() {
        assert_eq!(pycc_rt_range_continue(tag_smallint(0), tag_smallint(3), tag_smallint(1)), 1);
        assert_eq!(pycc_rt_range_continue(tag_smallint(3), tag_smallint(3), tag_smallint(1)), 0);
    }

    #[test]
    fn pycc_rt_range_continue_handles_negative_step() {
        assert_eq!(pycc_rt_range_continue(tag_smallint(3), tag_smallint(0), tag_smallint(-1)), 1);
        assert_eq!(pycc_rt_range_continue(tag_smallint(0), tag_smallint(0), tag_smallint(-1)), 0);
    }

    #[test]
    #[should_panic(expected = "must not be zero")]
    fn pycc_rt_range_continue_with_a_zero_step_panics() {
        // Calls the private `range_continue`, not the public
        // `pycc_rt_range_continue` wrapper -- see the implementation-note
        // comment above `range_continue`'s definition (same rationale as
        // `pycc_rt_int_add_panics_...` above).
        range_continue(tag_smallint(0), tag_smallint(3), tag_smallint(0));
    }

    #[test]
    fn pycc_rt_int_to_float_converts_the_untagged_value() {
        assert_eq!(pycc_rt_int_to_float(tag_smallint(5)), 5.0);
        assert_eq!(pycc_rt_int_to_float(tag_smallint(-3)), -3.0);
    }

    #[test]
    fn pycc_rt_float_floordiv_matches_python_floor_semantics() {
        assert_eq!(pycc_rt_float_floordiv(7.0, 2.0), 3.0);
        assert_eq!(pycc_rt_float_floordiv(-7.0, 2.0), -4.0);
    }

    #[test]
    fn pycc_rt_float_floormod_matches_python_floor_semantics() {
        assert_eq!(pycc_rt_float_floormod(-7.0, 2.0), 1.0);
        assert_eq!(pycc_rt_float_floormod(7.0, 2.0), 1.0);
    }

    #[test]
    fn pycc_rt_float_pow_computes_the_correct_power() {
        assert_eq!(pycc_rt_float_pow(2.0, 10.0), 1024.0);
        assert_eq!(pycc_rt_float_pow(9.0, 0.5), 3.0);
    }
}
