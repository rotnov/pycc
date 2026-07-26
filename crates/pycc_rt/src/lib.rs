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

use std::cell::Cell;

fn format_i64_line(value: i64) -> String {
    format!("{value}\n")
}

/// See D-061: every `Ty::Int` value is one LLVM `i64`. Its low bit is the
/// discriminant -- `1` means the high 63 bits (arithmetic-shift-recovered)
/// are the real value; `0` means the full 64 bits are a heap `BigInt`
/// pointer. `tag_bigint` (Task 9) constructs the `0` case on arithmetic
/// overflow; `bigint_ref`/`to_sign_and_magnitude` interpret it.
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

/// D-058: hand-rolled sign-magnitude limbs, base 2^32, little-endian,
/// no trailing zero limbs except a single `[0]` representing zero
/// itself. Never freed (leaked) -- unlike `PyStrObj`, D-060 only commits
/// `str` to real refcounting; a bigint is a rare, overflow-only path
/// with no v0.1 construct that could leak it in a hot loop the way an
/// unbounded string-building loop could (this is a deliberate, narrower
/// "simplest safe default" than `str`'s, recorded alongside D-061).
struct BigIntObj {
    negative: bool,
    limbs: Vec<u32>,
}

fn trim(limbs: &[u32]) -> Vec<u32> {
    let mut end = limbs.len();
    while end > 1 && limbs[end - 1] == 0 {
        end -= 1;
    }
    limbs[..end].to_vec()
}

fn magnitude_cmp(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
    let (a, b) = (trim(a), trim(b));
    if a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
    }
    std::cmp::Ordering::Equal
}

fn magnitude_add(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(a.len().max(b.len()) + 1);
    let mut carry: u64 = 0;
    for i in 0..a.len().max(b.len()) {
        let av = *a.get(i).unwrap_or(&0) as u64;
        let bv = *b.get(i).unwrap_or(&0) as u64;
        let sum = av + bv + carry;
        result.push((sum & 0xFFFF_FFFF) as u32);
        carry = sum >> 32;
    }
    if carry > 0 {
        result.push(carry as u32);
    }
    result
}

/// Requires `a >= b` (checked by every caller via `magnitude_cmp` first).
fn magnitude_sub(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut result = Vec::with_capacity(a.len());
    let mut borrow: i64 = 0;
    for (i, &av) in a.iter().enumerate() {
        let av = av as i64;
        let bv = *b.get(i).unwrap_or(&0) as i64;
        let mut diff = av - bv - borrow;
        if diff < 0 {
            diff += 1i64 << 32;
            borrow = 1;
        } else {
            borrow = 0;
        }
        result.push(diff as u32);
    }
    result
}

fn bigint_add_signed(a_neg: bool, a_mag: &[u32], b_neg: bool, b_mag: &[u32]) -> BigIntObj {
    if a_neg == b_neg {
        BigIntObj {
            negative: a_neg,
            limbs: trim(&magnitude_add(a_mag, b_mag)),
        }
    } else {
        match magnitude_cmp(a_mag, b_mag) {
            std::cmp::Ordering::Equal => BigIntObj {
                negative: false,
                limbs: vec![0],
            },
            std::cmp::Ordering::Greater => BigIntObj {
                negative: a_neg,
                limbs: trim(&magnitude_sub(a_mag, b_mag)),
            },
            std::cmp::Ordering::Less => BigIntObj {
                negative: b_neg,
                limbs: trim(&magnitude_sub(b_mag, a_mag)),
            },
        }
    }
}

fn bigint_from_i128(v: i128) -> BigIntObj {
    let negative = v < 0;
    let mut mag = v.unsigned_abs();
    let mut limbs = Vec::new();
    while mag > 0 {
        limbs.push((mag & 0xFFFF_FFFF) as u32);
        mag >>= 32;
    }
    if limbs.is_empty() {
        limbs.push(0);
    }
    BigIntObj { negative, limbs }
}

fn tag_bigint(b: BigIntObj) -> i64 {
    Box::into_raw(Box::new(b)) as i64
}

/// # Safety
/// `tagged` must be a `BigIntObj` pointer (an even bit pattern -- D-061);
/// every call site below checks `!is_smallint(tagged)` first.
unsafe fn bigint_ref<'a>(tagged: i64) -> &'a BigIntObj {
    unsafe { &*(tagged as *const BigIntObj) }
}

fn to_sign_and_magnitude(tagged: i64) -> (bool, Vec<u32>) {
    if is_smallint(tagged) {
        let v = untag_smallint(tagged);
        let negative = v < 0;
        let mag = v.unsigned_abs();
        (
            negative,
            trim(&[(mag & 0xFFFF_FFFF) as u32, (mag >> 32) as u32]),
        )
    } else {
        let b = unsafe { bigint_ref(tagged) };
        (b.negative, b.limbs.clone())
    }
}

fn divmod_small(limbs: &[u32], divisor: u32) -> (Vec<u32>, u32) {
    let mut quotient = vec![0u32; limbs.len()];
    let mut remainder: u64 = 0;
    for i in (0..limbs.len()).rev() {
        let acc = (remainder << 32) | limbs[i] as u64;
        quotient[i] = (acc / divisor as u64) as u32;
        remainder = acc % divisor as u64;
    }
    (quotient, remainder as u32)
}

fn bigint_to_decimal_string(negative: bool, limbs: &[u32]) -> String {
    let mut limbs = limbs.to_vec();
    let mut digits = Vec::new();
    loop {
        let (q, r) = divmod_small(&limbs, 10);
        digits.push(
            std::char::from_digit(r, 10).expect("a remainder of division by 10 is always 0-9"),
        );
        limbs = trim(&q);
        if limbs.len() == 1 && limbs[0] == 0 {
            break;
        }
    }
    if negative {
        digits.push('-');
    }
    digits.iter().rev().collect()
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
    if is_smallint(a) && is_smallint(b) {
        if let Some(result) = untag_smallint(a)
            .checked_add(untag_smallint(b))
            .and_then(fits_smallint)
        {
            return result;
        }
        // Both operands fit 63 bits, so their true sum always fits i128
        // with room to spare -- exact, no further bigint math needed
        // for this specific promotion step.
        return tag_bigint(bigint_from_i128(
            untag_smallint(a) as i128 + untag_smallint(b) as i128,
        ));
    }
    let (a_neg, a_mag) = to_sign_and_magnitude(a);
    let (b_neg, b_mag) = to_sign_and_magnitude(b);
    tag_bigint(bigint_add_signed(a_neg, &a_mag, b_neg, &b_mag))
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
    if is_smallint(a) && is_smallint(b) {
        if let Some(result) = untag_smallint(a)
            .checked_sub(untag_smallint(b))
            .and_then(fits_smallint)
        {
            return result;
        }
        return tag_bigint(bigint_from_i128(
            untag_smallint(a) as i128 - untag_smallint(b) as i128,
        ));
    }
    let (a_neg, a_mag) = to_sign_and_magnitude(a);
    let (b_neg, b_mag) = to_sign_and_magnitude(b);
    tag_bigint(bigint_add_signed(a_neg, &a_mag, !b_neg, &b_mag))
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_sub(a: i64, b: i64) -> i64 {
    int_sub(a, b)
}

fn int_mul(a: i64, b: i64) -> i64 {
    require_smallint(a, "multiplying");
    require_smallint(b, "multiplying");
    // Two tagged operands are each at most 62 magnitude bits, so their exact
    // product always fits in i128. Keep the tagged fast path when possible and
    // promote only the result, matching add/sub without requiring general
    // bigint multiplication yet.
    let product = untag_smallint(a) as i128 * untag_smallint(b) as i128;
    i64::try_from(product)
        .ok()
        .and_then(fits_smallint)
        .unwrap_or_else(|| tag_bigint(bigint_from_i128(product)))
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
    // unreachable dead code under D-061's fixed tagged representation:
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
    // reachable promotion case: floor-dividing the minimum taggable value
    // by `-1` negates it, producing exactly one more than the maximum
    // taggable magnitude (see
    // `pycc_rt_int_floordiv_promotes_the_negated_minimum_taggable_value`).
    let q = a / b;
    let r = a % b;
    let floored = if r != 0 && (r < 0) != (b < 0) {
        q - 1
    } else {
        q
    };
    fits_smallint(floored).unwrap_or_else(|| tag_bigint(bigint_from_i128(floored as i128)))
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
    // D-061's fixed tagged representation this is unreachable for the
    // same reason `int_floordiv`'s removed guard was (see its comment):
    // an already-tagged operand's untagged form can never equal
    // `i64::MIN`. Floor-mod's *result* can't overflow the taggable range
    // either -- unlike floor-division, which the comment on
    // `int_floordiv` explains can: floor-mod's result always satisfies
    // `|result| < |b|`, and every already-tagged `b` satisfies `|b| <=
    // 2^62` (D-061's 63-bit range), so `floored` always re-fits and the
    // `fits_smallint` round-trip check the brief had here (like
    // `int_floordiv`'s) is provably always-`Some` -- confirmed by
    // `cargo llvm-cov`: its `None` arm never executed under any test.
    // `tag_smallint` alone is therefore correct and simpler.
    let r = a % b;
    let floored = if r != 0 && (r < 0) != (b < 0) {
        r + b
    } else {
        r
    };
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
    if is_smallint(tagged) {
        pycc_rt_print_i64(untag_smallint(tagged));
        return;
    }
    let s = int_to_str(tagged);
    println!("{}", String::from_utf8_lossy(unsafe { &*s }.bytes()));
    unsafe { pycc_rt_str_decref(s) };
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
    if is_smallint(tagged) {
        return i8::from(untag_smallint(tagged) != 0);
    }
    // A bigint can now legitimately be zero (Task 9's `bigint_add_signed`
    // "equal magnitude, opposite sign" case), so the old "any bigint tag
    // is truthy" shortcut (Task 4, before real bigint values existed) is
    // no longer correct -- must inspect the actual magnitude.
    let b = unsafe { bigint_ref(tagged) };
    i8::from(!(b.limbs.len() == 1 && b.limbs[0] == 0))
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
    let (i, stop, step) = (
        untag_smallint(i),
        untag_smallint(stop),
        untag_smallint(step),
    );
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

/// Converts a tagged `int` (D-061) to its `f64` value -- the `int` half of
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

/// Python true division rejects both positive and negative zero divisors.
/// Until v0.3's exception machinery exists, a runtime panic becomes an
/// explicit process failure at the plain-C ABI boundary.
fn float_div(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        panic!("pycc_rt: float division by zero");
    }
    a / b
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_float_div(a: f64, b: f64) -> f64 {
    float_div(a, b)
}

/// CPython-compatible float floor-division and modulo. Computing both from
/// the same adjusted remainder avoids the off-by-one quotient produced by a
/// naive `(a / b).floor()` for values such as `1.0 // 0.1`.
fn float_divmod(a: f64, b: f64) -> (f64, f64) {
    if b == 0.0 {
        panic!("pycc_rt: float division or modulo by zero");
    }

    let mut modulo = a % b;
    let mut div = (a - modulo) / b;
    if modulo != 0.0 {
        if (b < 0.0) != (modulo < 0.0) {
            modulo += b;
            div -= 1.0;
        }
    } else {
        modulo = 0.0_f64.copysign(b);
    }

    let floordiv = if div != 0.0 {
        let mut floored = div.floor();
        if div - floored > 0.5 {
            floored += 1.0;
        }
        floored
    } else {
        0.0_f64.copysign(a / b)
    };
    (floordiv, modulo)
}

/// Python's `//` on `float`: floors toward negative infinity and snaps the
/// quotient the same way CPython does after floating-point remainder error.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_float_floordiv(a: f64, b: f64) -> f64 {
    float_divmod(a, b).0
}

/// Python's `%` on `float`: result takes the divisor's sign, including
/// signed zero.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_float_floormod(a: f64, b: f64) -> f64 {
    float_divmod(a, b).1
}

/// Python's `**` on `float`: a negative exponent is ordinary for a nonzero
/// base (`2.0 ** -1 == 0.5`), but Rust's `powf` silently returns infinities or
/// NaNs for cases where Python raises or produces a complex value. Until
/// exceptions and complex numbers exist, reject those domains explicitly.
fn float_pow(a: f64, b: f64) -> f64 {
    // CPython delegates non-finite exponent/base domains to libm: for
    // example, `(-1.0) ** inf == 1.0`, `0.0 ** -inf == inf`, and
    // `(-inf) ** 0.5 == inf`. The explicit exception/complex guards apply
    // only to finite operands; `fract()` is NaN for an infinite or NaN
    // exponent and would otherwise misclassify those ordinary real results.
    if b.is_finite() {
        if a == 0.0 && b < 0.0 {
            panic!("pycc_rt: zero cannot be raised to a negative float power");
        }
        if a.is_finite() && a < 0.0 && b.fract() != 0.0 {
            panic!(
                "pycc_rt: a negative float base with a fractional exponent requires complex support"
            );
        }
    }
    let result = a.powf(b);
    if a.is_finite() && b.is_finite() && result.is_infinite() {
        panic!("pycc_rt: float power overflow");
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_float_pow(a: f64, b: f64) -> f64 {
    float_pow(a, b)
}

/// D-059's `str` representation: up to 22 bytes are stored inline directly
/// in the `PyStrObj` allocation itself (no separate heap allocation for the
/// byte payload); anything longer heap-allocates a second, separate byte
/// buffer. Either way, `PyStrObj` itself (its refcount included) is always
/// exactly one heap allocation -- `pycc_codegen` never sees anything but an
/// opaque pointer to it (the same ABI-avoidance principle D-061 already
/// applies to `int`'s `BigInt`: no struct ever crosses the LLVM/Rust
/// boundary by value).
enum PyStrPayload {
    /// `(bytes, len)` -- only the first `len` bytes of `bytes` are
    /// meaningful; the rest is unused padding.
    Inline([u8; 22], u8),
    Heap(Box<[u8]>),
}

// `pub`, not private: every `pycc_rt_str_*` function below is a public
// (`#[unsafe(no_mangle)] pub extern "C" fn`) FFI entry point taking/
// returning `*mut PyStrObj`, and rustc's `private_interfaces` lint (a hard
// error under this project's `-D warnings` clippy gate) correctly refuses a
// private type in a public signature. `PyStrObj` stays fully opaque to any
// real Rust caller anyway: both fields below stay private, so nothing
// outside this module can construct one or read its contents except
// through these functions -- the "opaque pointer" contract this file's own
// doc comments describe is a privacy-of-fields property, not a
// privacy-of-the-type-name one.
pub struct PyStrObj {
    rc: Cell<u32>,
    payload: PyStrPayload,
}

impl PyStrObj {
    fn bytes(&self) -> &[u8] {
        match &self.payload {
            PyStrPayload::Inline(buf, len) => &buf[..*len as usize],
            PyStrPayload::Heap(b) => b,
        }
    }
}

/// Allocates a fresh `PyStrObj` with refcount `1`, choosing the inline or
/// heap `PyStrPayload` per D-059's 22-byte threshold. Shared by every
/// `pycc_rt_str_*` entry point below that constructs a brand-new string (a
/// literal, or a concatenation result) rather than merely operating on
/// already-existing ones.
fn new_pystr(bytes: &[u8]) -> *mut PyStrObj {
    let payload = if bytes.len() <= 22 {
        let mut buf = [0u8; 22];
        buf[..bytes.len()].copy_from_slice(bytes);
        PyStrPayload::Inline(buf, bytes.len() as u8)
    } else {
        PyStrPayload::Heap(bytes.to_vec().into_boxed_slice())
    };
    Box::into_raw(Box::new(PyStrObj {
        rc: Cell::new(1),
        payload,
    }))
}

/// Builds a `str` object from a compile-time literal's bytes
/// (`pycc_codegen`'s `MirExpr::StringLiteral` codegen, Task 7). Unlike every
/// `pycc_rt_int_*` arithmetic/comparison function, this has no failure mode
/// to guard against (allocation failure aborts via Rust's global allocator
/// rather than unwinding) -- so, per this crate's established convention
/// (see the implementation note above `int_add`, and `pycc_rt_int_truthy`'s
/// own doc comment for the same reasoning), this does not need the
/// private-logic/public-wrapper split: nothing here ever unwinds, so there's
/// no abort-vs-catch distinction for a caller to trip over. The same
/// rationale applies to every other `pycc_rt_str_*` function below.
///
/// # Safety
/// `ptr` must point to at least `len` readable bytes -- true for every
/// `pycc_codegen`-emitted call site, which always passes a compile-time
/// string literal's own constant global and byte length together.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_str_from_literal(ptr: *const u8, len: i64) -> *mut PyStrObj {
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    new_pystr(bytes)
}

/// Concatenates two `str` objects into a brand-new one (Python's `+` on
/// `str`, Task 7) -- never mutates either operand.
///
/// # Safety
/// `a`/`b` must be live `PyStrObj` pointers -- every `pycc_codegen` call
/// site only ever passes a value it just evaluated from a well-typed
/// `Ty::Str` expression.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_str_concat(a: *mut PyStrObj, b: *mut PyStrObj) -> *mut PyStrObj {
    let a_bytes = unsafe { &*a }.bytes();
    let b_bytes = unsafe { &*b }.bytes();
    let mut combined = Vec::with_capacity(a_bytes.len() + b_bytes.len());
    combined.extend_from_slice(a_bytes);
    combined.extend_from_slice(b_bytes);
    new_pystr(&combined)
}

/// Lexicographic byte-wise ordering (Task 7's `str` comparison codegen) --
/// `-1`/`0`/`1`, matching `pycc_rt_int_cmp`'s own convention.
///
/// # Safety
/// Same as `pycc_rt_str_concat`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_str_cmp(a: *mut PyStrObj, b: *mut PyStrObj) -> i32 {
    match unsafe { &*a }.bytes().cmp(unsafe { &*b }.bytes()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// `str`'s truthiness for `if`/`while` conditions (Task 7, mirrors
/// `pycc_rt_int_truthy`'s own doc comment): `False` only for the empty
/// string, matching CPython.
///
/// # Safety
/// `s` must be a live `PyStrObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_str_truthy(s: *mut PyStrObj) -> i8 {
    i8::from(!unsafe { &*s }.bytes().is_empty())
}

/// D-060's unconditional refcounting for `str`: increments `s`'s refcount by
/// one. A no-op on a null pointer -- not something Task 7's own codegen
/// ever actually passes, but a documented, tested part of this function's
/// contract nonetheless (mirroring how a null check is cheap insurance
/// against any future caller that binds a `str` local before it's ever
/// assigned).
///
/// # Safety
/// `s` must be either a null pointer or a live `PyStrObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_str_incref(s: *mut PyStrObj) {
    if s.is_null() {
        return;
    }
    let obj = unsafe { &*s };
    obj.rc.set(obj.rc.get() + 1);
}

/// D-060's unconditional refcounting for `str`: decrements `s`'s refcount by
/// one, freeing the allocation once it reaches zero. A no-op on a null
/// pointer, same rationale as `pycc_rt_str_incref` above.
///
/// # Safety
/// Same as `pycc_rt_str_incref`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_str_decref(s: *mut PyStrObj) {
    if s.is_null() {
        return;
    }
    let new_rc = unsafe { &*s }.rc.get() - 1;
    if new_rc == 0 {
        drop(unsafe { Box::from_raw(s) });
    } else {
        unsafe { &*s }.rc.set(new_rc);
    }
}

// --- Implementation note / deviation from the task brief -------------
//
// The brief's own version of `pycc_rt_int_to_str`/`pycc_rt_float_to_str`
// gave each straight to a single `#[unsafe(no_mangle)] pub extern "C" fn`
// (no private-logic/public-wrapper split). Both can panic --
// `int_to_str` via `require_smallint`'s bigint-rejection path (not yet
// exercised by any test in *this* task, since Task 9 is what first makes a
// bigint-valued tagged `int` reachable, but still a real panicking path
// today), `float_to_str` via its own scientific-notation-range rejection
// (directly exercised by this task's own tests) -- and both are plain
// `extern "C" fn`s, not `extern "C-unwind"`. Per this file's own
// established convention (see the implementation note above `int_add`,
// and this crate's project-wide rule that any new `pycc_rt` `extern "C"
// fn` that can panic needs this split), calling either public wrapper
// directly from ordinary same-crate Rust test code would abort the whole
// test binary (`SIGABRT`) instead of letting `#[should_panic]` catch an
// ordinary unwind -- confirmed empirically: `pycc_rt_float_to_str_rejects_
// magnitudes_needing_scientific_notation` aborted the test binary before
// this split existed. Fixed the same way as `int_add`/`pycc_rt_int_add`:
// a private, ordinary-Rust-ABI function holding the real logic (freely
// panics, unwinds normally) and a thin `#[unsafe(no_mangle)] pub extern
// "C" fn` wrapper of the exact name/signature the brief specifies.
// `bool_to_str` has no failure mode (same reasoning as `pycc_rt_str_from_
// literal`'s own doc comment) so it keeps the brief's single-function
// shape unchanged.

/// Formats a tagged `int` the way CPython's own `str(n)` would (Task 8) --
/// reused unchanged by f-string interpolation and Task 10's `print`. Shares
/// `format_i64_line`'s digit-formatting logic with `pycc_rt_print_i64`
/// rather than duplicating it, trimming the trailing newline that function
/// adds for its own (unrelated) purpose.
fn int_to_str(tagged: i64) -> *mut PyStrObj {
    if is_smallint(tagged) {
        return new_pystr(
            format_i64_line(untag_smallint(tagged))
                .trim_end()
                .as_bytes(),
        );
    }
    let b = unsafe { bigint_ref(tagged) };
    new_pystr(bigint_to_decimal_string(b.negative, &b.limbs).as_bytes())
}

/// See the panic-across-FFI note above `pycc_rt_int_add`: this crosses no
/// FFI boundary on its own successful-return tests, but a real
/// bigint-valued `tagged` (Task 9) would panic here, so this stays a thin
/// wrapper around `int_to_str`'s own freely-panicking logic rather than
/// housing that logic directly.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_to_str(tagged: i64) -> *mut PyStrObj {
    int_to_str(tagged)
}

/// Formats a `bool` the way CPython's own `str(b)` would: capitalized
/// `"True"`/`"False"`, never Rust's own lowercase `Display` spelling (Task
/// 8, reused unchanged by f-string interpolation and Task 10's `print`).
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_bool_to_str(value: i8) -> *mut PyStrObj {
    new_pystr(if value != 0 { b"True" } else { b"False" })
}

/// Formats a `float` the way CPython's own `str(f)` would (Task 8, reused
/// unchanged by f-string interpolation and Task 10's `print`).
///
/// Verified against `python3.13`'s actual `repr(float)`/`str(float)`
/// (identical since Python 3.1): CPython switches to scientific
/// notation once the value's magnitude is `>= 1e16` or (nonzero and)
/// `< 1e-4`; within that range it always shows at least one digit after
/// the decimal point (`3.0`, never bare `3`, unlike Rust's own `{}`
/// `Display` for `f64`), and `inf`/`-inf`/`nan` are lowercase (Rust's
/// own `Display` capitalizes `NaN`). Reproducing CPython's scientific
/// notation formatting exactly is out of scope for this task -- an
/// honest panic for that narrow range, not a silently wrong digit
/// string (a documented, named gap, same convention as D-026/D-043).
fn float_to_str(value: f64) -> *mut PyStrObj {
    if value.is_nan() {
        return new_pystr(b"nan");
    }
    if value.is_infinite() {
        return new_pystr(if value > 0.0 { b"inf" } else { b"-inf" });
    }
    let magnitude = value.abs();
    // clippy's `manual_range_contains`: `!(1e-4..1e16).contains(&magnitude)`
    // is `magnitude < 1e-4 || magnitude >= 1e16` (a `Range` is
    // start-inclusive, end-exclusive) -- the same condition as the task
    // brief's own `magnitude >= 1e16 || magnitude < 1e-4`, just reordered.
    if magnitude != 0.0 && !(1e-4..1e16).contains(&magnitude) {
        panic!(
            "pycc_rt: formatting a float this large or small ({value}) needs \
             scientific notation, which is not supported yet"
        );
    }
    let text = format!("{value}");
    let text = if text.contains('.') {
        text
    } else {
        format!("{text}.0")
    };
    new_pystr(text.as_bytes())
}

/// See the panic-across-FFI note above `pycc_rt_int_add`.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_float_to_str(value: f64) -> *mut PyStrObj {
    float_to_str(value)
}

/// Writes a `PyStrObj`'s bytes to stdout with no trailing newline (Task 10)
/// -- `print`'s new fully-general dispatch converts every argument to a
/// `str` via `to_str` first (reusing `pycc_rt_int_to_str`/`float_to_str`/
/// `bool_to_str`) and writes each one with this, separated by
/// `pycc_rt_print_space` and finished by `pycc_rt_print_newline`. Distinct
/// from Task 3's `pycc_rt_int_print`, which stays newline-inclusive and
/// int-only and is no longer called by `pycc_codegen`'s print dispatch
/// (still exercised by its own direct unit tests below). Never panics --
/// `String::from_utf8_lossy` cannot fail -- so this needs no
/// private-logic/public-wrapper split (same reasoning as `pycc_rt_str_from_
/// literal`'s own doc comment).
///
/// Deviation from the task brief: the brief's own version of this function
/// signature was a plain (non-`unsafe`) `pub extern "C" fn`, matching its
/// dereference of `s` (`*s`) inside its own internal `unsafe { }` block.
/// That doesn't compile clean under this crate's `-D warnings` clippy gate
/// -- `clippy::not_unsafe_ptr_arg_deref` (`#[deny]`d by default) rejects
/// exactly this shape: a public function taking a raw pointer and
/// dereferencing it without the function itself being `unsafe`. Every other
/// function in this file that dereferences a `*mut PyStrObj`
/// (`pycc_rt_str_from_literal`/`_concat`/`_cmp`/`_truthy`/`_incref`/
/// `_decref`) is already `pub unsafe extern "C" fn` for exactly this
/// reason; fixed the same way here, and documented with its own `# Safety`
/// section per that same established convention.
///
/// # Safety
/// `s` must be a live `*mut PyStrObj` previously returned by one of this
/// crate's own str-producing functions (same contract as
/// `pycc_rt_str_incref`/`pycc_rt_str_decref`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_print_write_str(s: *mut PyStrObj) {
    print!("{}", String::from_utf8_lossy(unsafe { &*s }.bytes()));
}

/// Prints a single space with no newline (Task 10) -- `print`'s separator
/// between arguments, matching CPython's default `sep=" "`.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_print_space() {
    print!(" ");
}

/// Prints `print`'s single trailing newline (Task 10), matching CPython's
/// default `end="\n"`.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_print_newline() {
    println!();
}

/// Prints the literal `None`, capitalized, with no trailing newline (Task
/// 10) -- CPython's `str(None)` -- for the narrow `print(f(...))` shape
/// where `f` returns `Ty::None` (see this task's own scope note: no v0.1
/// expression can construct a `None` *value* other than this exact
/// call-result shape, so there is no `PyStrObj`-producing `none_to_str` to
/// route through `pycc_rt_print_write_str` instead).
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_print_none() {
    print!("None");
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
    fn pycc_rt_int_sub_computes_the_correct_tagged_difference() {
        let a = tag_smallint(5);
        let b = tag_smallint(3);
        assert_eq!(untag_smallint(pycc_rt_int_sub(a, b)), 2);
    }

    #[test]
    fn pycc_rt_int_mul_computes_the_correct_tagged_product() {
        let a = tag_smallint(6);
        let b = tag_smallint(7);
        assert_eq!(untag_smallint(pycc_rt_int_mul(a, b)), 42);
    }

    #[test]
    fn pycc_rt_int_mul_promotes_a_product_outside_the_smallint_range() {
        let product = pycc_rt_int_mul(tag_smallint(i64::MAX >> 1), tag_smallint(2));
        assert!(!is_smallint(product));
        let text = pycc_rt_int_to_str(product);
        assert_eq!(unsafe { &*text }.bytes(), b"9223372036854775806");
        unsafe { pycc_rt_str_decref(text) };
    }

    #[test]
    fn pycc_rt_int_floordiv_matches_python_floor_semantics() {
        // Python: -7 // 2 == -4 (floors toward negative infinity), not -3
        // (truncation toward zero, which is what a raw LLVM/Rust `/` gives).
        assert_eq!(
            untag_smallint(pycc_rt_int_floordiv(tag_smallint(-7), tag_smallint(2))),
            -4
        );
        assert_eq!(
            untag_smallint(pycc_rt_int_floordiv(tag_smallint(7), tag_smallint(2))),
            3
        );
        assert_eq!(
            untag_smallint(pycc_rt_int_floordiv(tag_smallint(7), tag_smallint(-2))),
            -4
        );
    }

    #[test]
    fn pycc_rt_int_floormod_matches_python_floor_semantics() {
        // Python: -7 % 2 == 1 (result takes the divisor's sign), not -1.
        assert_eq!(
            untag_smallint(pycc_rt_int_floormod(tag_smallint(-7), tag_smallint(2))),
            1
        );
        assert_eq!(
            untag_smallint(pycc_rt_int_floormod(tag_smallint(7), tag_smallint(2))),
            1
        );
        assert_eq!(
            untag_smallint(pycc_rt_int_floormod(tag_smallint(7), tag_smallint(-2))),
            -1
        );
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
        assert_eq!(
            untag_smallint(pycc_rt_int_floordiv(tag_smallint(min), tag_smallint(1))),
            min
        );
    }

    #[test]
    fn pycc_rt_int_floordiv_promotes_the_negated_minimum_taggable_value() {
        // The actual reachable promotion case for `int_floordiv` (see its
        // comment): floor-dividing the minimum taggable value by `-1`
        // negates it, producing exactly one more than the maximum taggable
        // magnitude -- a real overflow of the tagged result, distinct from (and
        // the reason the brief's original i64::MIN/-1 *input* guard turned
        // out to be dead code).
        let min = i64::MIN >> 1;
        let quotient = int_floordiv(tag_smallint(min), tag_smallint(-1));
        assert!(!is_smallint(quotient));
        let text = pycc_rt_int_to_str(quotient);
        assert_eq!(unsafe { &*text }.bytes(), b"4611686018427387904");
        unsafe { pycc_rt_str_decref(text) };
    }

    #[test]
    fn pycc_rt_int_pow_computes_the_correct_tagged_power() {
        assert_eq!(
            untag_smallint(pycc_rt_int_pow(tag_smallint(2), tag_smallint(10))),
            1024
        );
        assert_eq!(
            untag_smallint(pycc_rt_int_pow(tag_smallint(5), tag_smallint(0))),
            1
        );
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
        // Bit pattern `0` (even) is what D-061 reserves for a heap `BigInt`
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
    fn pycc_rt_int_truthy_is_false_only_for_zero() {
        assert_eq!(pycc_rt_int_truthy(tag_smallint(0)), 0);
        assert_eq!(pycc_rt_int_truthy(tag_smallint(1)), 1);
        assert_eq!(pycc_rt_int_truthy(tag_smallint(-1)), 1);
    }

    #[test]
    fn pycc_rt_range_continue_handles_positive_step() {
        assert_eq!(
            pycc_rt_range_continue(tag_smallint(0), tag_smallint(3), tag_smallint(1)),
            1
        );
        assert_eq!(
            pycc_rt_range_continue(tag_smallint(3), tag_smallint(3), tag_smallint(1)),
            0
        );
    }

    #[test]
    fn pycc_rt_range_continue_handles_negative_step() {
        assert_eq!(
            pycc_rt_range_continue(tag_smallint(3), tag_smallint(0), tag_smallint(-1)),
            1
        );
        assert_eq!(
            pycc_rt_range_continue(tag_smallint(0), tag_smallint(0), tag_smallint(-1)),
            0
        );
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
        assert_eq!(pycc_rt_float_floordiv(1.0, 0.1), 9.0);

        // Exercises CPython's quotient snap-up correction: the intermediate
        // quotient is -5603572390.000001, whose `floor()` alone is one low.
        assert_eq!(
            pycc_rt_float_floordiv(
                f64::from_bits(0x8ec2_3615_82f2_e770),
                f64::from_bits(0x0cbb_eab0_bc9a_0e0c),
            ),
            -5_603_572_390.0
        );

        let negative_zero = pycc_rt_float_floordiv(-0.0, 2.0);
        assert_eq!(negative_zero, 0.0);
        assert!(negative_zero.is_sign_negative());
    }

    #[test]
    fn pycc_rt_float_div_computes_a_nonzero_division() {
        assert_eq!(pycc_rt_float_div(7.0, 2.0), 3.5);
    }

    #[test]
    #[should_panic(expected = "float division by zero")]
    fn float_div_rejects_a_zero_divisor() {
        float_div(1.0, -0.0);
    }

    #[test]
    #[should_panic(expected = "float division or modulo by zero")]
    fn float_divmod_rejects_a_zero_divisor() {
        float_divmod(1.0, 0.0);
    }

    #[test]
    fn pycc_rt_float_floormod_matches_python_floor_semantics() {
        assert_eq!(pycc_rt_float_floormod(-7.0, 2.0), 1.0);
        assert_eq!(pycc_rt_float_floormod(7.0, 2.0), 1.0);
        let negative_zero = pycc_rt_float_floormod(8.0, -2.0);
        assert_eq!(negative_zero, 0.0);
        assert!(negative_zero.is_sign_negative());
    }

    #[test]
    fn pycc_rt_float_pow_computes_the_correct_power() {
        assert_eq!(pycc_rt_float_pow(2.0, 10.0), 1024.0);
        assert_eq!(pycc_rt_float_pow(9.0, 0.5), 3.0);
        assert_eq!(pycc_rt_float_pow(2.0, -1.0), 0.5);
        assert_eq!(pycc_rt_float_pow(-1.0, f64::INFINITY), 1.0);
        assert_eq!(pycc_rt_float_pow(-2.0, f64::NEG_INFINITY), 0.0);
        assert_eq!(pycc_rt_float_pow(0.0, f64::NEG_INFINITY), f64::INFINITY);
        assert_eq!(pycc_rt_float_pow(f64::NEG_INFINITY, 0.5), f64::INFINITY);
        assert!(pycc_rt_float_pow(-1.0, f64::NAN).is_nan());
    }

    #[test]
    #[should_panic(expected = "zero cannot be raised")]
    fn float_pow_rejects_zero_to_a_negative_power() {
        float_pow(0.0, -1.0);
    }

    #[test]
    #[should_panic(expected = "requires complex support")]
    fn float_pow_rejects_a_negative_base_with_a_fractional_exponent() {
        float_pow(-1.0, 0.5);
    }

    #[test]
    #[should_panic(expected = "float power overflow")]
    fn float_pow_rejects_a_finite_overflow() {
        float_pow(f64::MAX, 2.0);
    }

    #[test]
    fn a_short_literal_round_trips_through_the_inline_representation() {
        // Every `pycc_rt_str_*` function is `unsafe extern "C"` (see their
        // own doc comments' `# Safety` sections) -- a genuinely public,
        // pointer-taking FFI entry point, unlike every plain-`i64`
        // `pycc_rt_int_*` function above, so calling one directly (even
        // from this crate's own same-binary Rust tests) needs an explicit
        // `unsafe` block, upholding each call's own documented precondition.
        unsafe {
            let bytes = b"hi";
            let s = pycc_rt_str_from_literal(bytes.as_ptr(), bytes.len() as i64);
            assert_eq!((*s).bytes(), b"hi");
            pycc_rt_str_decref(s);
        }
    }

    #[test]
    fn a_long_literal_round_trips_through_the_heap_representation() {
        unsafe {
            let long = "x".repeat(23); // one byte past the 22-byte inline cap (D-059)
            let s = pycc_rt_str_from_literal(long.as_ptr(), long.len() as i64);
            assert_eq!((*s).bytes(), long.as_bytes());
            pycc_rt_str_decref(s);
        }
    }

    #[test]
    fn concat_joins_bytes_from_both_operands() {
        unsafe {
            let a = pycc_rt_str_from_literal(b"foo".as_ptr(), 3);
            let b = pycc_rt_str_from_literal(b"bar".as_ptr(), 3);
            let joined = pycc_rt_str_concat(a, b);
            assert_eq!((*joined).bytes(), b"foobar");
            pycc_rt_str_decref(a);
            pycc_rt_str_decref(b);
            pycc_rt_str_decref(joined);
        }
    }

    #[test]
    fn cmp_orders_strings_lexicographically() {
        unsafe {
            let a = pycc_rt_str_from_literal(b"apple".as_ptr(), 5);
            let b = pycc_rt_str_from_literal(b"banana".as_ptr(), 6);
            assert_eq!(pycc_rt_str_cmp(a, a), 0);
            assert_eq!(pycc_rt_str_cmp(a, b), -1);
            assert_eq!(pycc_rt_str_cmp(b, a), 1);
            pycc_rt_str_decref(a);
            pycc_rt_str_decref(b);
        }
    }

    #[test]
    fn truthy_is_false_only_for_the_empty_string() {
        unsafe {
            let empty = pycc_rt_str_from_literal(b"".as_ptr(), 0);
            let non_empty = pycc_rt_str_from_literal(b"x".as_ptr(), 1);
            assert_eq!(pycc_rt_str_truthy(empty), 0);
            assert_eq!(pycc_rt_str_truthy(non_empty), 1);
            pycc_rt_str_decref(empty);
            pycc_rt_str_decref(non_empty);
        }
    }

    #[test]
    fn incref_then_decref_survives_until_the_final_decref() {
        unsafe {
            let s = pycc_rt_str_from_literal(b"hi".as_ptr(), 2);
            pycc_rt_str_incref(s); // rc 1 -> 2
            pycc_rt_str_decref(s); // rc 2 -> 1, must NOT free yet
            assert_eq!(pycc_rt_str_cmp(s, s), 0); // still safe to read
            pycc_rt_str_decref(s); // rc 1 -> 0, frees
        }
    }

    #[test]
    fn incref_and_decref_on_a_null_pointer_are_safe_no_ops() {
        unsafe {
            pycc_rt_str_incref(std::ptr::null_mut());
            pycc_rt_str_decref(std::ptr::null_mut());
        }
    }

    #[test]
    fn pycc_rt_int_to_str_formats_the_untagged_decimal_value() {
        // Deviation from the task brief: the brief's own version of this
        // (and every other new test below) called `pycc_rt_str_decref`
        // directly, unwrapped -- but that function is `pub unsafe extern
        // "C" fn` (see its own doc comment/safety section above), so an
        // unwrapped call doesn't compile (`E0133`, "call to unsafe function
        // ... is unsafe and requires unsafe block or unsafe function").
        // Every `pycc_rt_str_decref` call below is wrapped in its own
        // `unsafe { ... }` block, matching every other test in this file
        // that already calls an `unsafe fn` (e.g. `cmp_orders_strings_
        // lexicographically` above).
        let s = pycc_rt_int_to_str(tag_smallint(42));
        assert_eq!(unsafe { &*s }.bytes(), b"42");
        unsafe { pycc_rt_str_decref(s) };
        let s = pycc_rt_int_to_str(tag_smallint(-7));
        assert_eq!(unsafe { &*s }.bytes(), b"-7");
        unsafe { pycc_rt_str_decref(s) };
    }

    #[test]
    fn pycc_rt_bool_to_str_matches_python_s_capitalized_spelling() {
        let s = pycc_rt_bool_to_str(1);
        assert_eq!(unsafe { &*s }.bytes(), b"True");
        unsafe { pycc_rt_str_decref(s) };
        let s = pycc_rt_bool_to_str(0);
        assert_eq!(unsafe { &*s }.bytes(), b"False");
        unsafe { pycc_rt_str_decref(s) };
    }

    #[test]
    fn pycc_rt_float_to_str_always_shows_a_decimal_point() {
        // CPython: `str(3.0) == "3.0"`, not `"3"` -- unlike Rust's own `{}`
        // `Display` for `f64`, which omits the fractional part entirely for
        // a whole-number value.
        let s = pycc_rt_float_to_str(3.0);
        assert_eq!(unsafe { &*s }.bytes(), b"3.0");
        unsafe { pycc_rt_str_decref(s) };
        let s = pycc_rt_float_to_str(2.5);
        assert_eq!(unsafe { &*s }.bytes(), b"2.5");
        unsafe { pycc_rt_str_decref(s) };
        let s = pycc_rt_float_to_str(-0.5);
        assert_eq!(unsafe { &*s }.bytes(), b"-0.5");
        unsafe { pycc_rt_str_decref(s) };
    }

    #[test]
    fn pycc_rt_float_to_str_handles_infinity_and_nan_like_cpython() {
        // CPython: `str(float('inf')) == "inf"`, `str(float('nan')) == "nan"`
        // -- lowercase, unlike Rust's own `{}` (`"inf"`/`"NaN"`, capitalized
        // for `NaN`).
        let s = pycc_rt_float_to_str(f64::INFINITY);
        assert_eq!(unsafe { &*s }.bytes(), b"inf");
        unsafe { pycc_rt_str_decref(s) };
        let s = pycc_rt_float_to_str(f64::NEG_INFINITY);
        assert_eq!(unsafe { &*s }.bytes(), b"-inf");
        unsafe { pycc_rt_str_decref(s) };
        let s = pycc_rt_float_to_str(f64::NAN);
        assert_eq!(unsafe { &*s }.bytes(), b"nan");
        unsafe { pycc_rt_str_decref(s) };
    }

    #[test]
    #[should_panic(expected = "not supported yet")]
    fn pycc_rt_float_to_str_rejects_magnitudes_needing_scientific_notation() {
        // CPython's `repr(float)` switches to scientific notation outside a
        // specific decimal-exponent range (verified against `python3.13`:
        // `repr(1e17)` is `'1e+17'`, not the full 18-digit expansion) --
        // reproducing that exact algorithm is out of scope for this task;
        // this is an honest, loud "not supported yet" for that narrow range
        // (never a silently wrong digit string), not silently accepted.
        //
        // Deviation from the task brief: calls the private `float_to_str`
        // directly, not the public `pycc_rt_float_to_str` wrapper the brief
        // used -- the wrapper is a plain `extern "C" fn`, so a panic
        // crossing its boundary aborts the whole test binary (`SIGABRT`)
        // instead of unwinding into `#[should_panic]`'s own catch (see the
        // private-logic/public-wrapper split added above, same convention
        // as `int_add`/`pycc_rt_int_add`).
        float_to_str(1e17);
    }

    #[test]
    #[should_panic(expected = "not supported yet")]
    fn pycc_rt_float_to_str_rejects_small_magnitudes_needing_scientific_notation() {
        // Same rationale as the large-magnitude case above, for the *low*
        // end of the supported range: verified against `python3.13`,
        // `repr(1e-5)` is `'1e-05'` (scientific), unlike `repr(1e-4)` which
        // is `'0.0001'` (still positional). Neither the given tests above
        // nor `pycc_rt_float_to_str_always_shows_a_decimal_point`/
        // `_accepts_the_boundary_just_inside_the_supported_range` ever drive
        // `magnitude < 1e-4` to `true` -- every value they use is either
        // `>= 1e-4` in magnitude or exactly `0.0`, so without this test the
        // small-magnitude half of that `||` would never actually fire.
        // Calls the private `float_to_str` directly, same reason as the
        // large-magnitude test above.
        float_to_str(1e-5);
    }

    #[test]
    fn pycc_rt_float_to_str_accepts_the_boundary_just_inside_the_supported_range() {
        // Deviation from the task brief: the brief's own version of this
        // test called `pycc_rt_float_to_str(1e16)`, expecting
        // `b"10000000000000000.0"` -- but `1e16` is exactly the boundary
        // this function's own `magnitude >= 1e16` check *rejects* (verified
        // against `python3.13`: `repr(1e16)` is itself `'1e+16'`, scientific
        // notation, contradicting the brief test's expectation that it
        // stays positional). That test as written would either panic
        // (contradicting its own non-`#[should_panic]` assertion) or --
        // had the boundary check instead been written as `>` -- silently
        // accept a value CPython itself always renders in scientific
        // notation. Fixed to use the actual double just inside the
        // supported range: `9999999999999998.0`, the IEEE-754 `f64`
        // immediately below `1e16` (`ulp` there is `2.0`), which
        // `python3.13`'s own `repr`/`str` renders positionally as
        // `'9999999999999998.0'`.
        let s = pycc_rt_float_to_str(9999999999999998.0);
        assert_eq!(unsafe { &*s }.bytes(), b"9999999999999998.0");
        unsafe { pycc_rt_str_decref(s) };
    }

    #[test]
    fn pycc_rt_float_to_str_handles_zero_and_negative_zero() {
        // CPython: `str(0.0) == "0.0"`, `str(-0.0) == "-0.0"` (verified
        // against `python3.13`) -- neither the brief's own given tests nor
        // any test above ever passes a zero magnitude, so the `magnitude !=
        // 0.0` short-circuit guard's `false` outcome (skipping the
        // large/small-magnitude check entirely) was otherwise never
        // exercised.
        let s = pycc_rt_float_to_str(0.0);
        assert_eq!(unsafe { &*s }.bytes(), b"0.0");
        unsafe { pycc_rt_str_decref(s) };
        let s = pycc_rt_float_to_str(-0.0);
        assert_eq!(unsafe { &*s }.bytes(), b"-0.0");
        unsafe { pycc_rt_str_decref(s) };
    }

    #[test]
    fn adding_past_i64_range_now_promotes_instead_of_panicking() {
        // This is the exact fixture Task 3's
        // `pycc_rt_int_add_panics_on_overflow_before_bigint_promotion_exists`
        // used to require a panic for -- it must now succeed and print the
        // exact mathematical sum.
        let huge = pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(1));
        let s = pycc_rt_int_to_str(huge);
        assert_eq!(unsafe { &*s }.bytes(), b"4611686018427387904");
        unsafe { pycc_rt_str_decref(s) };
    }

    #[test]
    fn subtracting_past_the_negative_range_promotes_correctly() {
        let huge = pycc_rt_int_sub(tag_smallint(i64::MIN >> 1), tag_smallint(1));
        let s = pycc_rt_int_to_str(huge);
        assert_eq!(unsafe { &*s }.bytes(), b"-4611686018427387905");
        unsafe { pycc_rt_str_decref(s) };
    }

    #[test]
    fn repeated_addition_exercises_the_general_bigint_plus_smallint_path() {
        // Simulates unbounded `fib`-style growth. The first `pycc_rt_int_add`
        // call overflows the tagged range (`i64::MAX >> 1` is exactly the
        // largest value that still round-trips through tagging, so adding
        // it to itself does not fit) and promotes via the one-time
        // `i128`-widening path. Every one of the 20 subsequent additions
        // then has a `BigIntObj` as its left operand and a tagged smallint
        // as its right operand -- `is_smallint(a) && is_smallint(b)` is
        // false for all of them, so they all go through
        // `to_sign_and_magnitude`/`bigint_add_signed`'s general limb
        // arithmetic, not the fast path or the one-time promotion shortcut.
        // (The running total here stays well under 128 bits -- about 67 at
        // the end -- this test is not about exceeding `i128`'s own range,
        // only about exercising the general bigint-arithmetic code path
        // repeatedly rather than just once.)
        let mut acc = pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(i64::MAX >> 1));
        for _ in 0..20 {
            acc = pycc_rt_int_add(acc, tag_smallint(i64::MAX >> 1));
        }
        assert!(!is_smallint(acc));
        let s = pycc_rt_int_to_str(acc);
        let text = String::from_utf8(unsafe { &*s }.bytes().to_vec()).unwrap();
        assert_eq!(text, (bigint_reference_sum()).to_string());
        unsafe { pycc_rt_str_decref(s) };
    }

    /// Independent reference computation for the test above (22 additions
    /// of `i64::MAX >> 1`, well within `i128`'s own range) -- this doesn't
    /// exercise `pycc_rt`'s bigint code at all, so it's a trustworthy oracle
    /// for what the *correct* sum is, independent of any bug the code under
    /// test might have.
    fn bigint_reference_sum() -> i128 {
        let step = (i64::MAX >> 1) as i128;
        let mut acc = step + step;
        for _ in 0..20 {
            acc += step;
        }
        acc
    }

    #[test]
    fn a_bigint_that_would_fit_back_in_smallint_range_still_formats_correctly() {
        // Two already-promoted values that sum back to something small
        // (mathematically representable as a smallint) are not required to
        // shrink back down (D-061/this task's own "simplest correct" choice
        // -- once a value touches the bigint path, it stays represented as
        // one) -- but the printed *value* must still be exactly right.
        let a = pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(1)); // a bigint
        let b = pycc_rt_int_sub(tag_smallint(0), a); // -a, also a bigint (sub promotes too)
        let zero = pycc_rt_int_add(a, b);
        let s = pycc_rt_int_to_str(zero);
        assert_eq!(unsafe { &*s }.bytes(), b"0");
        unsafe { pycc_rt_str_decref(s) };
    }

    #[test]
    fn a_bigint_zero_is_falsy() {
        let a = pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(1));
        let b = pycc_rt_int_sub(tag_smallint(0), a);
        let zero = pycc_rt_int_add(a, b);
        assert_eq!(pycc_rt_int_truthy(zero), 0);
    }

    #[test]
    fn a_nonzero_bigint_is_truthy() {
        // Companion to `a_bigint_zero_is_falsy`: pins the *other* outcome of
        // `pycc_rt_int_truthy`'s bigint branch (a magnitude that is not a
        // single zero limb). `i64::MAX >> 1` is the largest tagged smallint,
        // so `+ 1` overflows the fixnum range and promotes to a real bigint.
        let huge = pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(1));
        assert!(
            !is_smallint(huge),
            "value must actually be a bigint, not a tagged smallint"
        );
        assert_eq!(pycc_rt_int_truthy(huge), 1);
    }

    #[test]
    fn a_bigint_print_path_runs_without_panicking() {
        // Exercises `int_print`'s bigint branch (`int_to_str` -> `println!`
        // -> `str_decref`). This in-process test only proves that path runs
        // without panicking; the "prints the decimal digits with a trailing
        // newline" behavior itself is asserted end-to-end by pycc_codegen's
        // `compiles_a_loop_whose_accumulator_overflows_into_a_bigint`, which
        // runs the compiled binary and checks its stdout is the digits plus
        // exactly one `\n` (capturing `println!` output inside a libtest unit
        // body would need stdout-redirect plumbing this crate does not have).
        let huge = pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(1));
        pycc_rt_int_print(huge);
    }

    #[test]
    fn adding_two_opposite_sign_bigints_with_the_same_limb_count_but_different_magnitude() {
        // Every other opposite-sign addition/subtraction test above either
        // has exactly equal magnitudes (immediately `Ordering::Equal`,
        // never reaching `magnitude_cmp`'s per-limb loop body) or magnitudes
        // needing a different number of limbs (resolved by `magnitude_cmp`'s
        // own length check before the loop runs at all) -- so this is the
        // only test exercising `magnitude_cmp`'s `a[i] != b[i]` true branch,
        // `bigint_add_signed`'s `Ordering::Greater` arm, and
        // `magnitude_sub`'s borrow path (`diff < 0`), all at once.
        //
        // `step = i64::MAX >> 1 = 0x3FFF_FFFF_FFFF_FFFF` (low limb
        // `0xFFFF_FFFF`, high limb `0x3FFF_FFFF`). `big = 2 * step`,
        // `bigger = 3 * step` -- both promote to real 2-limb bigints (their
        // magnitude exceeds a single `u32`'s range), and `bigger`'s low limb
        // (`0xFFFF_FFFD`) is smaller than `big`'s low limb (`0xFFFF_FFFE`),
        // so subtracting them genuinely borrows from the high limb.
        // `bigger + (-big)` must equal `step` exactly.
        let step = i64::MAX >> 1;
        let big = pycc_rt_int_add(tag_smallint(step), tag_smallint(step));
        let bigger = pycc_rt_int_add(big, tag_smallint(step));
        let neg_big = pycc_rt_int_sub(tag_smallint(0), big);
        assert!(!is_smallint(big) && !is_smallint(bigger) && !is_smallint(neg_big));
        let result = pycc_rt_int_add(bigger, neg_big);
        let s = pycc_rt_int_to_str(result);
        assert_eq!(unsafe { &*s }.bytes(), step.to_string().as_bytes());
        unsafe { pycc_rt_str_decref(s) };
    }

    #[test]
    fn bigint_from_i128_of_zero_still_has_a_single_zero_limb() {
        // Not reachable through `int_add`/`int_sub`'s own overflow-promotion
        // call sites -- a mathematically-zero sum/difference always fits
        // the tagged smallint range, so `checked_add`/`checked_sub` +
        // `fits_smallint` succeed first and this fallback is never invoked
        // with `0`. Tested directly against the private helper instead,
        // matching this file's own convention of testing a general-purpose
        // private function's own contract rather than only the narrower
        // paths its current callers happen to exercise.
        let b = bigint_from_i128(0);
        assert!(!b.negative);
        assert_eq!(b.limbs, vec![0]);
    }

    #[test]
    fn print_write_str_writes_bytes_with_no_trailing_newline() {
        // stdout is captured by the test harness; this only proves the call
        // itself doesn't panic/crash (same rationale as this file's other
        // direct extern-fn exercises). `pycc_rt_str_from_literal`/`pycc_rt_
        // str_decref`/`pycc_rt_print_write_str` (see that function's own
        // doc comment for why it's `unsafe` too, a fix over the task
        // brief's own version) are all `unsafe extern "C" fn`s, so every
        // call below needs an `unsafe` block -- the task brief's own test
        // listing omitted it entirely, a genuine compile-error bug, fixed
        // here the same way this file's other direct `pycc_rt_str_from_
        // literal` call sites already do it.
        unsafe {
            let s = pycc_rt_str_from_literal(b"hi".as_ptr(), 2);
            pycc_rt_print_write_str(s);
            pycc_rt_str_decref(s);
        }
    }

    #[test]
    fn print_space_and_newline_and_none_do_not_panic() {
        pycc_rt_print_space();
        pycc_rt_print_newline();
        pycc_rt_print_none();
    }
}
