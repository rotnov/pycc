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

mod instance;

fn format_i64_line(value: i64) -> String {
    format!("{value}\n")
}

/// See D-061/D-141: every int-compatible value is one LLVM `i64`. Odd words
/// are ordinary smallints; exact words `2` and `6` preserve `False` and
/// `True` identity after a bool-to-int boundary; non-zero words aligned to
/// four bytes are heap `BigInt` pointers. `classify_encoded_int` is the one
/// fail-closed classifier used before interpreting any even word.
const TAG_BIT: i64 = 1;
const LOW_TAG_MASK: i64 = 0b11;
const BOOL_FALSE_MARKER: i64 = 0b0010;
const BOOL_TRUE_MARKER: i64 = 0b0110;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EncodedIntKind {
    SmallInt,
    BoolFalse,
    BoolTrue,
    BigInt,
}

fn tag_smallint(value: i64) -> i64 {
    (value << 1) | TAG_BIT
}

fn untag_smallint(tagged: i64) -> i64 {
    tagged >> 1 // arithmetic (sign-extending) shift for `i64`
}

fn is_smallint(tagged: i64) -> bool {
    tagged & TAG_BIT == TAG_BIT
}

/// Classifies every word in the int-compatible ABI before any pointer cast.
/// Odd words are ordinary smallints; two exact low-tag-`10` words preserve
/// bool identity; non-zero aligned words are bigint pointers. Every other
/// pattern fails closed instead of being dereferenced as an attacker-chosen
/// pointer.
fn classify_encoded_int(encoded: i64) -> EncodedIntKind {
    if is_smallint(encoded) {
        EncodedIntKind::SmallInt
    } else if encoded == BOOL_FALSE_MARKER {
        EncodedIntKind::BoolFalse
    } else if encoded == BOOL_TRUE_MARKER {
        EncodedIntKind::BoolTrue
    } else if encoded != 0 && encoded & LOW_TAG_MASK == 0 {
        EncodedIntKind::BigInt
    } else {
        panic!("pycc_rt: invalid encoded int word {encoded:#x}")
    }
}

fn inline_int_value(encoded: i64) -> Option<i64> {
    match classify_encoded_int(encoded) {
        EncodedIntKind::SmallInt => Some(untag_smallint(encoded)),
        EncodedIntKind::BoolFalse => Some(0),
        EncodedIntKind::BoolTrue => Some(1),
        EncodedIntKind::BigInt => None,
    }
}

/// `None` when `value` needs the full 64 bits (including sign) to
/// represent -- i.e. tagging then untagging would not round-trip.
fn fits_smallint(value: i64) -> Option<i64> {
    let tagged = tag_smallint(value);
    (untag_smallint(tagged) == value).then_some(tagged)
}

fn require_inline_int(encoded: i64, context: &str) -> i64 {
    inline_int_value(encoded)
        .unwrap_or_else(|| panic!("pycc_rt: {context} a bigint-valued `int` is not supported yet"))
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

const _: () = assert!(std::mem::align_of::<BigIntObj>() >= 4);
const _: () = assert!(std::mem::size_of::<*const BigIntObj>() <= std::mem::size_of::<i64>());

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
/// `tagged` must classify as a `BigIntObj` pointer. Classification is
/// repeated here so no dereference can bypass the fail-closed tag check.
unsafe fn bigint_ref<'a>(tagged: i64) -> &'a BigIntObj {
    if classify_encoded_int(tagged) != EncodedIntKind::BigInt {
        panic!("pycc_rt: internal error: attempted bigint dereference of a non-pointer int word")
    }
    unsafe { &*(tagged as *const BigIntObj) }
}

fn to_sign_and_magnitude(tagged: i64) -> (bool, Vec<u32>) {
    if let Some(v) = inline_int_value(tagged) {
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
    if let (Some(a), Some(b)) = (inline_int_value(a), inline_int_value(b)) {
        if let Some(result) = a.checked_add(b).and_then(fits_smallint) {
            return result;
        }
        // Both operands fit 63 bits, so their true sum always fits i128
        // with room to spare -- exact, no further bigint math needed
        // for this specific promotion step.
        return tag_bigint(bigint_from_i128(a as i128 + b as i128));
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
    if let (Some(a), Some(b)) = (inline_int_value(a), inline_int_value(b)) {
        if let Some(result) = a.checked_sub(b).and_then(fits_smallint) {
            return result;
        }
        return tag_bigint(bigint_from_i128(a as i128 - b as i128));
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
    let a = require_inline_int(a, "multiplying");
    let b = require_inline_int(b, "multiplying");
    // Two decoded inline operands are each at most 62 magnitude bits, so
    // their exact product always fits in i128. Keep the tagged fast path when
    // possible and promote only the result, matching add/sub without
    // requiring general bigint multiplication yet.
    let product = a as i128 * b as i128;
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
    let a = require_inline_int(a, "dividing");
    let b = require_inline_int(b, "dividing");
    if b == 0 {
        panic!("pycc_rt: integer division by zero");
    }
    // Deviation from the task brief: the brief's own code guarded here
    // against the classic hardware trap on a raw `i64::MIN / -1` (the
    // mathematical quotient `2^63` doesn't fit `i64`, and Rust's checked
    // `/`/`%` themselves panic/trap on that exact pair). That guard is
    // unreachable dead code under D-061/D-141's encoded representation:
    // `a`/`b` here are already decoded from a valid inline int-compatible
    // argument. Smallints decode within D-061's 63-bit range and bool markers
    // decode to `0`/`1`, so neither can equal `i64::MIN`. `cargo llvm-cov`'s
    // region coverage confirmed
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
    let a = require_inline_int(a, "computing the modulo of");
    let b = require_inline_int(b, "computing the modulo of");
    if b == 0 {
        panic!("pycc_rt: integer modulo by zero");
    }
    // Deviation from the task brief: the brief's own code special-cased
    // `a == i64::MIN && b == -1` here (mirroring `int_floordiv`'s
    // original guard) to sidestep the same raw `%` hardware trap. Under
    // D-061/D-141 encoded representation this is unreachable for the
    // same reason `int_floordiv`'s removed guard was (see its comment):
    // a decoded inline operand can never equal
    // `i64::MIN`. Floor-mod's *result* can't overflow the taggable range
    // either -- unlike floor-division, which the comment on
    // `int_floordiv` explains can: floor-mod's result always satisfies
    // `|result| < |b|`, and every decoded inline `b` satisfies `|b| <=
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
    let _ = require_inline_int(base, "exponentiating");
    let mut exp = require_inline_int(exp, "exponentiating");
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
    let a = require_inline_int(a, "comparing");
    let b = require_inline_int(b, "comparing");
    match a.cmp(&b) {
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

/// `int`'s truthiness for `if`/`while` conditions (Task 4, D-141). Valid
/// smallints and bool-identity markers are decoded inline; valid bigint
/// pointers inspect their magnitude. A malformed encoded word is an internal
/// ABI violation and fails closed at this C boundary rather than being
/// dereferenced as a pointer.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_truthy(tagged: i64) -> i8 {
    if let Some(value) = inline_int_value(tagged) {
        return i8::from(value != 0);
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
// panic (`require_inline_int`'s bigint-rejection path, and the zero-step
// case below), so it needs the same split every other panicking
// `pycc_rt_int_*` function already gets: a private, ordinary-Rust-ABI
// `range_continue` holding the real logic (freely panics, unwinds
// normally, `#[should_panic]`-testable), and a thin `pub extern "C"`
// wrapper of the exact brief-specified name/signature for pycc-generated
// code to call. The zero-step test below calls `range_continue` directly,
// not the public wrapper, for the same reason every other
// `#[should_panic]` test in this file does.
fn range_continue(i: i64, stop: i64, step: i64) -> i8 {
    let (i, stop, step) = (
        require_inline_int(i, "iterating"),
        require_inline_int(stop, "iterating"),
        require_inline_int(step, "iterating"),
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

/// Converts an encoded int-compatible value (D-061/D-141) to `f64` -- the
/// `int` half of Python's `int`/`float` arithmetic promotion (Task 6). Can
/// panic (via `require_inline_int`'s bigint/invalid-word rejection path), so
/// -- per this crate's established convention, see the implementation note
/// above `int_add` --
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
    require_inline_int(tagged, "converting") as f64
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_to_float(tagged: i64) -> f64 {
    int_to_float(tagged)
}

/// D-141 checked decoder for an int-compatible ABI word. Ordinary smallints
/// decode to their numeric value, while the exact `False`/`True` markers
/// decode to `0`/`1`. Bigints and malformed words fail closed. Generated
/// code uses the numeric result for container indices, slice bounds, and
/// range normalization. Container value ingress calls this function only to
/// validate the word, then stores the original encoded word unchanged so a
/// bool marker keeps its runtime identity.
///
/// --- Implementation note / deviation from the task brief ---------------
///
/// The brief's own Step 4 code makes this a single plain `extern "C" fn`,
/// with a `#[should_panic]` test calling that same public wrapper directly
/// for the bigint case. Per this crate's established convention (see the
/// implementation-note comment above `int_add`, discovered empirically
/// during Task 3, and repeated for `range_continue` and `int_to_float`): a
/// panic that unwinds past a plain `extern "C" fn`'s own boundary is caught
/// right there and turned into a process abort, regardless of who calls it
/// -- including this crate's own same-binary Rust tests, which would
/// `SIGABRT` rather than let `#[should_panic]` catch anything. This
/// function can panic, so it gets the same split every other panicking
/// `pycc_rt_int_*` function already has: a private, ordinary-Rust-ABI
/// function holding the real logic, and a thin `pub extern "C"` wrapper of
/// the exact brief-specified name and signature for Task 11b's generated
/// code to call unchanged.
fn int_untag_checked(tagged: i64) -> i64 {
    match inline_int_value(tagged) {
        Some(value) => value,
        None => {
            panic!("pycc_rt: int boundary does not support bigint-valued values yet")
        }
    }
}

/// # Safety
/// None -- takes no pointer, only an `i64`. (The panic-across-FFI note on
/// `pycc_rt_int_add` applies to this wrapper as well.)
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_untag_checked(tagged: i64) -> i64 {
    int_untag_checked(tagged)
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

/// Python's `**` on `float`: unlike `int_pow`, a negative exponent is
/// perfectly ordinary here (`2.0 ** -1 == 0.5`). But three domains where
/// `f64::powf` silently returns an IEEE-754 special value diverge from
/// Python, which raises or produces a complex result instead (verified against
/// `python3.13`):
/// zero raised to a negative power (`ZeroDivisionError`), a negative base
/// raised to a non-integer power (a complex result -- `pycc` has no
/// complex type), and a finite base/exponent pair whose true result
/// overflows `float` range (`OverflowError`). Each is an honest panic
/// instead of a silently wrong `inf`/`NaN`, matching this crate's
/// division-by-zero convention above.
fn float_pow(a: f64, b: f64) -> f64 {
    // CPython delegates non-finite exponent/base domains to libm: for
    // example, `(-1.0) ** inf == 1.0`, `0.0 ** -inf == inf`, and
    // `(-inf) ** 0.5 == inf`. The explicit exception/complex guards apply
    // only to finite operands; `fract()` is NaN for an infinite or NaN
    // exponent and would otherwise misclassify those ordinary real results.
    if b.is_finite() {
        if a == 0.0 && b < 0.0 {
            panic!("pycc_rt: 0.0 cannot be raised to a negative power");
        }
        if a.is_finite() && a < 0.0 && b.fract() != 0.0 {
            panic!(
                "pycc_rt: a negative float raised to a non-integer power is not supported yet (would require a complex result)"
            );
        }
    }
    let result = a.powf(b);
    if result.is_infinite() && a.is_finite() && b.is_finite() {
        panic!("pycc_rt: float power overflowed (result too large to represent)");
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
// `int_to_str` via the encoded-int classifier's rejection path (not yet
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

/// Formats an encoded int-compatible value the way CPython's own `str(n)`
/// would (Task 8/D-141) --
/// reused unchanged by f-string interpolation and Task 10's `print`. Shares
/// `format_i64_line`'s digit-formatting logic with `pycc_rt_print_i64`
/// rather than duplicating it, trimming the trailing newline that function
/// adds for its own (unrelated) purpose.
fn int_to_str(tagged: i64) -> *mut PyStrObj {
    match classify_encoded_int(tagged) {
        EncodedIntKind::SmallInt => new_pystr(
            format_i64_line(untag_smallint(tagged))
                .trim_end()
                .as_bytes(),
        ),
        EncodedIntKind::BoolFalse => new_pystr(b"False"),
        EncodedIntKind::BoolTrue => new_pystr(b"True"),
        EncodedIntKind::BigInt => {
            let b = unsafe { bigint_ref(tagged) };
            new_pystr(bigint_to_decimal_string(b.negative, &b.limbs).as_bytes())
        }
    }
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

/// Prints the literal `None`, capitalized, with no trailing newline (Task 10)
/// -- CPython's `str(None)` -- for any supported materializable non-`print()`
/// `Ty::None` expression. This includes direct user-function, `ListAppend`,
/// and `SetAdd` results, D-075 parameter values, and values loaded from
/// ordinary assignment storage. `None` is an unboxed canonical unit carrier
/// rather than a `PyStrObj`, so there is no `none_to_str` allocation to route
/// through `pycc_rt_print_write_str` instead.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_print_none() {
    print!("None");
}

/// `list[int]`'s runtime object (D-105: v0.2 codegen only supports
/// `list[int]`; any other element type stays a `pycc_types` diagnostic,
/// T0034, never reaching codegen). Header shape follows `PyStrObj`'s real
/// precedent (`rc: Cell<u32>` plus payload) rather than `docs/RUNTIME.md`'s
/// former stale 16-byte generic-header spec (corrected by this plan's
/// Task 12) -- `PyStrObj` is this runtime's only other *refcounted* heap
/// object, and it never had a `type_id`/`flags` field either. (`BigIntObj`,
/// above, is a third heap-allocated type -- `Box::into_raw`'d by
/// `tag_bigint` -- but it carries no `rc` field at all: it is a deliberate,
/// documented leak, not refcounted like `PyStrObj`/`PyIntListObj`, so it is
/// not a counterexample to this struct's own refcounted-header shape.)
///
/// No `#[repr(C)]`: exactly like `PyStrObj` (see that struct's own doc
/// comment), `pycc_codegen` never sees anything but an opaque pointer to
/// this type and only ever calls the `pycc_rt_int_list_*` functions below
/// on it -- no struct ever crosses the LLVM/Rust boundary by value, so
/// there is no ABI layout to pin down.
///
/// The growable payload is a safe `Cell<Vec<i64>>`, not a raw pointer with
/// manual `std::alloc`/`realloc`/`dealloc` calls: `Vec<i64>` already gets
/// growth, bounds-safe indexing, and (critically) its own `Drop` glue for
/// free, the same way `PyStrObj`'s `Box<[u8]>` heap payload does. A
/// `RefCell<Vec<i64>>` was considered and rejected -- its runtime
/// borrow-checking panics on any overlapping borrow, a new panic mode this
/// object's callers (future LLVM-generated code with call patterns not
/// fully under this file's control) could trip with no relation to Python
/// semantics. `Cell::take`/`Cell::set` avoid that: `take` moves the `Vec`
/// out (leaving an empty one behind) without holding a borrow, the caller
/// mutates the owned value, then `set` moves it back in -- no borrow is
/// ever held across the mutation, so there is nothing for a runtime check
/// to reject.
///
/// `pub`, not private, for the same reason `PyStrObj` is `pub`: every
/// `pycc_rt_int_list_*` function below is a public FFI entry point
/// returning or taking `*mut PyIntListObj`, and rustc's
/// `private_interfaces` lint refuses a private type in a public
/// signature. Both fields stay private, so the "opaque pointer" contract
/// still holds for any real Rust caller.
///
/// **Element representation (D-141).** Each slot stores the int-compatible
/// encoded word unchanged: odd ordinary smallint, exact `2`/`6` bool marker,
/// or (once container bigints are supported) an aligned bigint pointer.
/// Current generated ingress validates every word with
/// `pycc_rt_int_untag_checked`, so bigint elements remain an explicit scope
/// cut, but it stores the original encoded word rather than the decoded
/// number. Reads, iteration, slicing, and pop therefore preserve `False`/
/// `True` identity. Lengths and indices remain raw implementation counters.
pub struct PyIntListObj {
    rc: Cell<u32>,
    items: Cell<Vec<i64>>,
}

/// Allocates a fresh, empty `PyIntListObj` with refcount `1`. Never
/// panics -- `Vec::new()` performs no allocation -- so, per this file's
/// established convention (see the implementation note above
/// `pycc_rt_int_add`), this needs no private-logic/public-wrapper split:
/// nothing here ever unwinds, so there is no abort-vs-catch distinction
/// for a caller to trip over.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_list_new() -> *mut PyIntListObj {
    Box::into_raw(Box::new(PyIntListObj {
        rc: Cell::new(1),
        items: Cell::new(Vec::new()),
    }))
}

/// Appends `value` to the end of `list` (Python's `list.append`, D-105's
/// v0.2 `list[int]` slice). Grows `list`'s backing `Vec` via its own
/// amortized-doubling `push`, so this never needs to reimplement
/// capacity-doubling by hand.
///
/// # Element representation
/// `value` is an encoded D-141 word and is stored exactly as given. The FFI
/// caller must first validate it with `pycc_rt_int_untag_checked`; generated
/// code does so and deliberately ignores the decoded result. This preserves
/// a bool marker while retaining the current explicit rejection of bigint
/// container elements.
///
/// # Safety
/// `list` must be a live `PyIntListObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_list_append(list: *mut PyIntListObj, value: i64) {
    let list = unsafe { &*list };
    let mut items = list.items.take();
    items.push(value);
    list.items.set(items);
}

/// # Safety (panic-across-FFI note, same rationale as `pycc_rt_int_add`'s
/// own doc comment above)
/// `pycc_rt_int_list_get` below is a plain `extern "C" fn`, not `extern
/// "C-unwind"`. A panic that would otherwise unwind past its boundary is
/// instead turned into a process abort -- correct for pycc-generated LLVM
/// code calling it, but unsuitable for `#[should_panic]` testing directly
/// against the public wrapper (confirmed empirically the same way
/// `pycc_rt_float_to_str`'s own split was: see this file's tests below).
/// This private function holds the real, freely-panicking logic; tests
/// exercising the panic call it directly, exactly as this file's
/// established convention already does for `int_add`/`float_to_str`.
fn int_list_get(list: &PyIntListObj, index: i64) -> i64 {
    let items = list.items.take();
    let len = items.len();
    if index < 0 || index as usize >= len {
        // Restore the payload before panicking. Not required for
        // correctness in this crate's actual single-shot FFI usage (a
        // panicking `pycc_rt_int_list_get` call aborts the whole process
        // via the wrapper below, so the object is never touched again
        // either way) but cheap insurance against leaving `list` with an
        // emptied-out payload for any future caller that does catch this
        // unwind (e.g. this file's own `#[should_panic]` tests).
        list.items.set(items);
        panic!("pycc_rt: list index out of range");
    }
    let value = items[index as usize];
    list.items.set(items);
    value
}

/// Reads the element at `index` (Python's `list[index]`, D-105's v0.2
/// `list[int]` slice). Panics on an out-of-range index, matching
/// `pycc_rt`'s established "honest panic over silently wrong data"
/// convention (see `float_to_str`'s own doc comment for the same
/// principle applied elsewhere in this file).
///
/// Known v0.2 scope cut: negative indices are not supported. Real Python
/// treats `lst[-1]` as the last element, but `pycc_types` has no way to
/// reject a negative index at compile time (the index value is only known
/// at runtime) -- so this panics on *any* negative index, the same
/// "index out of range" panic as a too-large positive one. This is a
/// deliberate, documented v0.2 gap (D-108), not a bug: it means a
/// conformance fixture exercising this function must not use negative
/// indexing, since that would panic here rather than matching CPython's
/// last-element behavior.
///
/// # Element representation
/// `index` is a raw container offset; generated code obtains it by decoding
/// an int-compatible expression with `pycc_rt_int_untag_checked`. The return
/// value is the stored D-141 encoded word unchanged and needs no conversion
/// before being used as an ordinary `Ty::Int` expression.
///
/// # Safety
/// `list` must be a live `PyIntListObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_list_get(list: *mut PyIntListObj, index: i64) -> i64 {
    int_list_get(unsafe { &*list }, index)
}

/// Returns `list`'s current element count (Python's `len(list)`, D-105's
/// v0.2 `list[int]` slice).
///
/// # Element representation
/// The returned count is a **raw, untagged** `i64`, not a D-061-tagged
/// one (see `PyIntListObj`'s own doc comment) -- it is a plain `usize`
/// element count, unrelated to any stored element's own representation.
/// `len(x)` is itself a `Ty::Int`-typed expression result in ordinary
/// Python semantics, so a caller that uses this return value as a
/// `Ty::Int` anywhere else in generated code (e.g. passing it to
/// `pycc_rt_int_to_str` for `print(len(x))`) must `tag_smallint` it
/// first. Stored-element reads, unlike this raw count, already return an
/// encoded word and need no conversion.
///
/// # Safety
/// `list` must be a live `PyIntListObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_list_len(list: *mut PyIntListObj) -> i64 {
    let list = unsafe { &*list };
    let items = list.items.take();
    let len = items.len() as i64;
    list.items.set(items);
    len
}

/// D-060-style unconditional refcounting for `list[int]`, matching
/// `pycc_rt_str_incref`'s own convention exactly: increments `list`'s
/// refcount by one, a no-op on a null pointer.
///
/// # Safety
/// `list` must be either a null pointer or a live `PyIntListObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_list_incref(list: *mut PyIntListObj) {
    if list.is_null() {
        return;
    }
    let obj = unsafe { &*list };
    obj.rc.set(obj.rc.get() + 1);
}

/// D-060-style unconditional refcounting for `list[int]`, matching
/// `pycc_rt_str_decref`'s own convention exactly: decrements `list`'s
/// refcount by one, freeing the allocation once it reaches zero (which,
/// via `Box::from_raw`'s own drop glue, also frees the `Cell<Vec<i64>>`
/// payload's backing buffer -- no separate manual deallocation call
/// needed). A no-op on a null pointer, same rationale as
/// `pycc_rt_int_list_incref` above.
///
/// # Safety
/// Same as `pycc_rt_int_list_incref`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_list_decref(list: *mut PyIntListObj) {
    if list.is_null() {
        return;
    }
    let new_rc = unsafe { &*list }.rc.get() - 1;
    if new_rc == 0 {
        drop(unsafe { Box::from_raw(list) });
    } else {
        unsafe { &*list }.rc.set(new_rc);
    }
}

/// # Safety (panic-across-FFI note, same rationale as `int_list_get`'s own
/// doc comment above)
/// `pycc_rt_int_list_slice` below is a plain `extern "C" fn`, not `extern
/// "C-unwind"` -- a panic that would otherwise unwind past its boundary is
/// instead turned into a process abort. This private function holds the
/// real, freely-panicking logic; tests exercising a panic call it directly,
/// exactly like `int_list_get`'s own split.
fn int_list_slice(list: &PyIntListObj, start: i64, stop: i64, step: i64) -> *mut PyIntListObj {
    if start < 0 {
        panic!("pycc_rt: slice start must be non-negative");
    }
    if stop < 0 {
        panic!("pycc_rt: slice stop must be non-negative");
    }
    if step <= 0 {
        panic!("pycc_rt: slice step must be positive");
    }
    let items = list.items.take();
    let len = items.len() as i64;
    let clamped_start = start.min(len);
    let clamped_stop = stop.min(len);
    let result = pycc_rt_int_list_new();
    // Unlike `int_list_get` (whose own doc comment restores `list`'s
    // payload before panicking on an out-of-range index), this loop's
    // take-window contains no fallible operation, so there is nothing to
    // restore: `items[i as usize]` cannot panic, since `i` starts at
    // `clamped_start` and the loop guard keeps it below `clamped_stop`,
    // and both clamped bounds are `.min(len)`-derived, so
    // `clamped_start <= i < clamped_stop <= len == items.len()` holds on
    // every iteration. `i += step` cannot overflow `i64` either: `i` never
    // exceeds `len`, and `step` itself already passed
    // `pycc_rt_int_untag_checked`'s 63-bit smallint range check at every
    // real `pycc_codegen` call site, so `i + step` stays far below
    // `i64::MAX`.
    let mut i = clamped_start;
    while i < clamped_stop {
        unsafe { pycc_rt_int_list_append(result, items[i as usize]) };
        i += step;
    }
    list.items.set(items);
    result
}

/// Returns a **new** list containing the clamped, strided sub-range
/// `[start, stop)` of `list`'s elements, stepping by `step` (Python's
/// `list[start:stop:step]`, D-118's v0.2 `list[int]` slice). Panics on a
/// negative `start`/`stop` or a non-positive `step` -- v0.2 ships no
/// CPython-style negative-index/negative-step semantics, extending D-108's
/// own uniform "no negative addressing" scope cut (`pycc_rt_int_list_get`)
/// to slicing. `start`/`stop` are clamped into `[0, len]` after the sign
/// check, matching CPython's own out-of-range-slice-bound clamping --
/// required for the accepted subset (omitted/over-long bounds) to match
/// CPython byte-for-byte, not merely a nicety. The three sign/positivity
/// panics run before `list.items.take()`, mirroring `int_list_get`'s own
/// "leave `list` intact on a panic" care -- a panicking call here never
/// touches `list`'s payload at all, so there is nothing to restore.
///
/// # Element representation
/// `start`/`stop`/`step` are raw, untagged `i64` offsets/strides, not
/// D-061-tagged `Ty::Int` values -- a caller with a tagged operand must
/// `pycc_rt_int_untag_checked` each one first, exactly like
/// `pycc_rt_int_list_get`'s own `index` parameter. The returned list's own
/// elements are copied through unchanged (already D-141 encoded words per
/// `PyIntListObj`'s representation) -- no per-element
/// conversion happens here, exactly like a single-element read.
///
/// # Safety
/// `list` must be a live `PyIntListObj` pointer. Takes `*mut` rather than
/// the task plan's own sketched `*const`, matching every other
/// `pycc_rt_int_list_*` function in this file (`_get`/`_len` both take
/// `*mut PyIntListObj` even though they too only read `list`'s payload) --
/// a deliberate consistency choice over the plan's literal text, not a
/// functional difference (LLVM's opaque pointer type distinguishes neither
/// side at the `pycc_codegen` call site either way).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_list_slice(
    list: *mut PyIntListObj,
    start: i64,
    stop: i64,
    step: i64,
) -> *mut PyIntListObj {
    int_list_slice(unsafe { &*list }, start, stop, step)
}

/// # Safety (panic-across-FFI note, same rationale as `int_list_get`'s own
/// doc comment above)
/// `pycc_rt_int_list_pop` below is a plain `extern "C" fn`, not `extern
/// "C-unwind"` -- a panic that would otherwise unwind past its boundary is
/// instead turned into a process abort. This private function holds the
/// real, freely-panicking logic; tests exercising the panic call it
/// directly, exactly like `int_list_get`'s own split.
fn int_list_pop(list: &PyIntListObj) -> i64 {
    let mut items = list.items.take();
    let Some(value) = items.pop() else {
        // Restore the (empty) payload before panicking, same rationale as
        // `int_list_get`'s own "restore before panicking" comment.
        list.items.set(items);
        panic!("pycc_rt: pop from empty list");
    };
    list.items.set(items);
    value
}

/// Removes and returns the list's **last** element (Python's `list.pop()`,
/// PR-12, D-119). Panics if `list` is empty, matching this file's
/// established "honest panic over silently wrong data" convention (CPython
/// raises a catchable `IndexError` here; this compiler has no exception
/// model, so this is an unrecoverable panic instead). The panic message is
/// `"pycc_rt: pop from empty list"`.
///
/// # Element representation
/// The returned value is the stored D-141 encoded word unchanged, so a bool
/// marker keeps its runtime identity and no output-side conversion is needed.
///
/// # Safety
/// `list` must be a live `PyIntListObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_list_pop(list: *mut PyIntListObj) -> i64 {
    int_list_pop(unsafe { &*list })
}

/// `dict[str, int]`'s runtime representation (D-121): a dense,
/// insertion-ordered array of `(key, value)` pairs. `Cell<Vec<...>>`
/// mirrors `PyIntListObj`'s own choice over `RefCell` -- `Cell::take`/
/// `_::set` never holds a borrow across a mutation, so there is no new
/// runtime-panic mode from overlapping borrows. Not `#[repr(C)]` -- never
/// crosses the LLVM/Rust boundary by value, only as an opaque pointer
/// (mirrors `PyStrObj`/`PyIntListObj`). Lookup is linear-scan comparison
/// via `pycc_rt_str_cmp` (D-121), not a hash table. Values are D-141 encoded
/// words preserved unchanged across set/get/get-or-default; generated ingress
/// validates them with `pycc_rt_int_untag_checked` before storage.
pub struct PyDictObj {
    rc: Cell<u32>,
    entries: Cell<Vec<(*mut PyStrObj, i64)>>,
}

/// Allocates a fresh, empty `PyDictObj` with refcount `1`. Never panics.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_dict_new() -> *mut PyDictObj {
    Box::into_raw(Box::new(PyDictObj {
        rc: Cell::new(1),
        entries: Cell::new(Vec::new()),
    }))
}

/// Insert-or-update (D-123): if `key` compares equal (`pycc_rt_str_cmp`)
/// to an already-stored key, that entry's value is overwritten in place,
/// preserving insertion order; otherwise `(key, value)` is appended.
/// Leak-only (D-124): the stored key pointer is neither increfed on
/// insert nor decrefed on update-in-place or ever.
///
/// # Safety
/// `dict` and `key` must be live pointers from `pycc_rt_dict_new`/
/// `pycc_rt_str_new`-family functions respectively.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_dict_set(dict: *mut PyDictObj, key: *mut PyStrObj, value: i64) {
    let mut entries = unsafe { &*dict }.entries.take();
    match entries
        .iter()
        .position(|(k, _)| unsafe { pycc_rt_str_cmp(*k, key) } == 0)
    {
        Some(i) => entries[i].1 = value,
        None => entries.push((key, value)),
    }
    unsafe { &*dict }.entries.set(entries);
}

fn dict_get(dict: &PyDictObj, key: *mut PyStrObj) -> i64 {
    let entries = dict.entries.take();
    let found = entries
        .iter()
        .find(|(k, _)| unsafe { pycc_rt_str_cmp(*k, key) } == 0)
        .map(|(_, v)| *v);
    dict.entries.set(entries);
    found.unwrap_or_else(|| panic!("pycc_rt: dict key not found"))
}

/// Linear-scan lookup (D-121). Panics if no stored key compares equal to
/// `key` -- this compiler has no `KeyError` handling, so a missing key is
/// an honest panic rather than a silently wrong value. The panic message
/// is `"pycc_rt: dict key not found"`.
///
/// # Safety
/// Same as `pycc_rt_dict_set`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_dict_get(dict: *mut PyDictObj, key: *mut PyStrObj) -> i64 {
    dict_get(unsafe { &*dict }, key)
}

fn dict_get_or_default(dict: &PyDictObj, key: *mut PyStrObj, default: i64) -> i64 {
    let entries = dict.entries.take();
    let found = entries
        .iter()
        .find(|(k, _)| unsafe { pycc_rt_str_cmp(*k, key) } == 0)
        .map(|(_, v)| *v);
    dict.entries.set(entries);
    found.unwrap_or(default)
}

/// Returns the value stored for `key`, or `default` if `key` is absent
/// (Python's `dict.get(key, default)`, PR-12, D-119) -- unlike
/// `pycc_rt_dict_get`, this **never panics** on a missing key; that is the
/// entire point of the two-argument form. `default` is passed through
/// unchanged, so this function itself never panics either -- it is a total
/// function given the type gate (`pycc_types`' T0021/T0033) already
/// enforces `key`/`default`'s types.
///
/// # Safety
/// Same as `pycc_rt_dict_get`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_dict_get_or_default(
    dict: *mut PyDictObj,
    key: *mut PyStrObj,
    default: i64,
) -> i64 {
    dict_get_or_default(unsafe { &*dict }, key, default)
}

/// Number of entries. `Cell::take` followed by re-`set`ting it, mirroring
/// `pycc_rt_int_list_len`'s own exact pattern (never leave the `Cell`
/// empty on return).
///
/// # Safety
/// `dict` must be a live `PyDictObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_dict_len(dict: *mut PyDictObj) -> i64 {
    let entries = unsafe { &*dict }.entries.take();
    let len = entries.len() as i64;
    unsafe { &*dict }.entries.set(entries);
    len
}

/// Private half of `pycc_rt_dict_key_at` below, same panic-across-FFI
/// split as `int_list_get`/`dict_get` above: a plain `extern "C" fn`
/// turns an unwinding panic into a process abort, so the freely-panicking
/// logic lives here and tests exercising the panic call this directly.
///
/// Uses `Vec::get` (bounds-checked, never panics on its own) rather than
/// indexing directly, so the payload can be restored into the `Cell`
/// *before* the panic branch -- mirroring `int_list_get`'s own "restore
/// before panicking" comment and `dict_get`'s own `unwrap_or_else` shape,
/// instead of `dict.entries.take()` followed by a direct index that would
/// leave the `Cell` holding an empty `Vec` if it panicked. Not required
/// for this compiler's actual generated code (`ForDict`'s own loop bound
/// is always `pycc_rt_dict_len`, so no call site this project's own
/// codegen emits can go out of range), but cheap insurance against
/// leaving `dict` with a permanently emptied payload for any future
/// caller that does catch this unwind (e.g. this file's own
/// `#[should_panic]` test below).
fn dict_key_at(dict: &PyDictObj, index: i64) -> *mut PyStrObj {
    let entries = dict.entries.take();
    let key = entries.get(index as usize).map(|(k, _)| *k);
    dict.entries.set(entries);
    key.unwrap_or_else(|| panic!("pycc_rt: dict_key_at index out of range"))
}

/// Key at a given insertion-order position, used only by `ForDict`'s own
/// iteration codegen (Task 5) -- never a user-facing indexing operation
/// (dict has no positional index in Python). Panics (message
/// `"pycc_rt: dict_key_at index out of range"`) if `index` is out of
/// range; unreachable in this compiler's own generated code (see `#
/// Safety` below) but not undefined behavior if ever hit.
///
/// # Safety
/// `dict` must be a live `PyDictObj` pointer; `index` must satisfy
/// `0 <= index < pycc_rt_dict_len(dict)` (an internal codegen invariant,
/// not user input -- `ForDict`'s own loop bound is `pycc_rt_dict_len`,
/// so an out-of-range call here would be a codegen bug, not a possible
/// user program).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_dict_key_at(dict: *mut PyDictObj, index: i64) -> *mut PyStrObj {
    dict_key_at(unsafe { &*dict }, index)
}

/// Unconditional refcounting (D-124), mirroring `pycc_rt_int_list_incref`
/// exactly. No-op on null.
///
/// # Safety
/// `dict` must be null or a live `PyDictObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_dict_incref(dict: *mut PyDictObj) {
    if dict.is_null() {
        return;
    }
    let obj = unsafe { &*dict };
    obj.rc.set(obj.rc.get() + 1);
}

/// Mirrors `pycc_rt_int_list_decref` exactly: frees via `Box::from_raw`
/// once `rc` hits 0. Not called from any `pycc_codegen` site in this PR
/// (D-124, leak-only).
///
/// # Safety
/// `dict` must be null or a live `PyDictObj` pointer not used again after
/// its refcount reaches 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_dict_decref(dict: *mut PyDictObj) {
    if dict.is_null() {
        return;
    }
    let obj = unsafe { &*dict };
    let rc = obj.rc.get() - 1;
    obj.rc.set(rc);
    if rc == 0 {
        drop(unsafe { Box::from_raw(dict) });
    }
}

/// `set[int]`'s runtime representation (D-121/D-141): structurally identical
/// to `PyIntListObj` (a dense array of encoded int-compatible words), but insertion goes
/// through `pycc_rt_int_set_add`'s own dedup check (linear scan, D-121)
/// instead of `PyIntListObj`'s unconditional append -- this is the one
/// behavioral difference and the reason this is its own distinct type
/// rather than a reuse of `PyIntListObj` (mirrors the same reasoning
/// D-107 gave for `Scalar::List` needing its own variant instead of
/// reusing `Scalar::Str`: distinct semantics deserve a distinct type so
/// the compiler enforces every call site acknowledges the difference).
pub struct PyIntSetObj {
    rc: Cell<u32>,
    items: Cell<Vec<i64>>,
}

/// Allocates a fresh, empty `PyIntSetObj` with refcount `1`. Never panics.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_set_new() -> *mut PyIntSetObj {
    Box::into_raw(Box::new(PyIntSetObj {
        rc: Cell::new(1),
        items: Cell::new(Vec::new()),
    }))
}

/// Dedup-checked insert (D-121/D-141): linear-scan by decoded Python numeric
/// value; appends only if absent and preserves the first encoded word. Thus
/// `{True, 1}` retains `True`, while `{1, True}` retains ordinary integer `1`.
///
/// # Safety
/// `set` must be a live `PyIntSetObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_set_add(set: *mut PyIntSetObj, value: i64) {
    let mut items = unsafe { &*set }.items.take();
    let value_numeric = require_inline_int(value, "storing in set[int]");
    if !items
        .iter()
        .copied()
        .any(|existing| require_inline_int(existing, "reading from set[int]") == value_numeric)
    {
        items.push(value);
    }
    unsafe { &*set }.items.set(items);
}

/// Returns `set`'s current element count.
///
/// # Safety
/// `set` must be a live `PyIntSetObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_set_len(set: *mut PyIntSetObj) -> i64 {
    let items = unsafe { &*set }.items.take();
    let len = items.len() as i64;
    unsafe { &*set }.items.set(items);
    len
}

/// Panics if `current_len` differs from `expected_len`. `ForSet`'s own
/// iteration codegen (Task 9) calls this once per loop-test evaluation,
/// comparing a freshly re-read `pycc_rt_int_set_len` against the length
/// captured once in the loop's preheader. `set.add(value)` (PR-12, D-119)
/// made this reachable for the first time: `for x in s: s.add(x + 1)`
/// would otherwise silently visit every newly-inserted element too,
/// never terminating for a value like `x + 1` that is always distinct
/// from every prior element -- unlike `ForDict`'s own identical
/// re-read-every-iteration shape, which D-123 already accepts as a
/// bounded divergence (a dict grown by re-inserting existing keys stays
/// finite; a set grown by always-novel derived values does not). Real
/// CPython raises a catchable `RuntimeError: Set changed size during
/// iteration` here; this compiler has no exception model, so an honest
/// panic is the correct match for this file's own established
/// convention, not a new failure mode invented for this case.
fn check_set_len_unchanged(current_len: i64, expected_len: i64) {
    if current_len != expected_len {
        panic!("pycc_rt: set changed size during iteration");
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_int_set_check_not_resized(current_len: i64, expected_len: i64) {
    check_set_len_unchanged(current_len, expected_len);
}

/// Element at a given insertion-order position, used only by `ForSet`'s
/// own iteration codegen (Task 9) -- `set` has no user-facing indexing in
/// Python (real CPython also rejects `s[0]`), so this is an internal
/// codegen helper only.
///
/// # Safety
/// `set` must be a live `PyIntSetObj` pointer; `index` must satisfy
/// `0 <= index < pycc_rt_int_set_len(set)` (an internal codegen
/// invariant, mirroring `pycc_rt_dict_key_at`'s own contract).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_set_get(set: *mut PyIntSetObj, index: i64) -> i64 {
    let items = unsafe { &*set }.items.take();
    let value = items[index as usize];
    unsafe { &*set }.items.set(items);
    value
}

/// D-060-style unconditional refcounting for `set[int]`, matching
/// `pycc_rt_int_list_incref`'s own convention exactly: increments `set`'s
/// refcount by one, a no-op on a null pointer.
///
/// # Safety
/// `set` must be either a null pointer or a live `PyIntSetObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_set_incref(set: *mut PyIntSetObj) {
    if set.is_null() {
        return;
    }
    let obj = unsafe { &*set };
    obj.rc.set(obj.rc.get() + 1);
}

/// D-060-style unconditional refcounting for `set[int]`, matching
/// `pycc_rt_int_list_decref`'s own convention exactly: decrements `set`'s
/// refcount by one, freeing the allocation once it reaches zero (which,
/// via `Box::from_raw`'s own drop glue, also frees the `Cell<Vec<i64>>`
/// payload's backing buffer -- no separate manual deallocation call
/// needed). A no-op on a null pointer, same rationale as
/// `pycc_rt_int_set_incref` above.
///
/// # Safety
/// Same as `pycc_rt_int_set_incref`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_set_decref(set: *mut PyIntSetObj) {
    if set.is_null() {
        return;
    }
    let obj = unsafe { &*set };
    let rc = obj.rc.get() - 1;
    obj.rc.set(rc);
    if rc == 0 {
        drop(unsafe { Box::from_raw(set) });
    }
}

/// Runtime NameError for call-before-`def` (issue #22). pycc's type checker
/// rejects a call to a not-yet-`def`ined function in top-level code statically,
/// but a function body may call a sibling whose `def` has not executed yet at
/// the time the caller is invoked -- CPython raises `NameError` at that point,
/// and so does this. Until v0.3's exception machinery exists, a runtime panic
/// becomes an explicit process failure at the plain-C ABI boundary, matching
/// every other runtime error in this crate.
///
/// Takes a null-terminated C string (the function name) so the codegen can
/// pass a global string constant directly. The panic-across-FFI note on
/// `pycc_rt_int_add` applies: a panic that would unwind past this `extern "C"`
/// boundary is caught and turned into a process abort by Rust's default
/// panic-over-FFI behavior, which is exactly the desired outcome for a
/// runtime NameError in a compiled program.
///
/// Split into a private freely-panicking function (testable with
/// `#[should_panic]`) and a thin `extern "C"` wrapper, matching the same
/// split every other panicking runtime function in this file uses.
fn name_error(name: *const std::os::raw::c_char) -> ! {
    let name_str = if name.is_null() {
        "<unknown>"
    } else {
        unsafe { std::ffi::CStr::from_ptr(name) }
            .to_str()
            .unwrap_or("<invalid utf-8>")
    };
    panic!("pycc_rt: NameError: name '{name_str}' is not defined");
}

#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_name_error(name: *const std::os::raw::c_char) -> ! {
    name_error(name)
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
    #[should_panic(expected = "pycc_rt: NameError: name 'foo' is not defined")]
    fn name_error_panics_with_function_name() {
        let name = std::ffi::CString::new("foo").unwrap();
        name_error(name.as_ptr());
    }

    #[test]
    #[should_panic(expected = "pycc_rt: NameError: name '<unknown>' is not defined")]
    fn name_error_on_null_pointer_uses_unknown() {
        name_error(std::ptr::null());
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
        let bigint = tag_bigint(bigint_from_i128(1i128 << 80));
        int_cmp(bigint, tag_smallint(1));
    }

    #[test]
    #[should_panic(expected = "invalid encoded int word 0x0")]
    fn encoded_int_classification_rejects_null_before_any_pointer_cast() {
        classify_encoded_int(0);
    }

    #[test]
    #[should_panic(expected = "invalid encoded int word 0xa")]
    fn encoded_int_classification_rejects_unrecognized_low_tag_10_words() {
        classify_encoded_int(0b1010);
    }

    #[test]
    #[should_panic(expected = "attempted bigint dereference of a non-pointer int word")]
    fn bigint_dereference_rechecks_the_encoded_word_before_casting() {
        // Even an unsafe internal caller cannot turn a valid bool marker
        // into a pointer without passing the central classifier again.
        unsafe {
            let _ = bigint_ref(BOOL_TRUE_MARKER);
        }
    }

    #[test]
    fn bool_identity_markers_decode_numerically_but_format_as_bools() {
        assert_eq!(int_untag_checked(BOOL_FALSE_MARKER), 0);
        assert_eq!(int_untag_checked(BOOL_TRUE_MARKER), 1);
        assert_eq!(pycc_rt_int_truthy(BOOL_FALSE_MARKER), 0);
        assert_eq!(pycc_rt_int_truthy(BOOL_TRUE_MARKER), 1);
        assert_eq!(pycc_rt_int_to_float(BOOL_FALSE_MARKER), 0.0);
        assert_eq!(pycc_rt_int_to_float(BOOL_TRUE_MARKER), 1.0);

        unsafe {
            let false_text = int_to_str(BOOL_FALSE_MARKER);
            let true_text = int_to_str(BOOL_TRUE_MARKER);
            assert_eq!((&*false_text).bytes(), b"False");
            assert_eq!((&*true_text).bytes(), b"True");
            pycc_rt_str_decref(false_text);
            pycc_rt_str_decref(true_text);
        }
    }

    #[test]
    fn arithmetic_consumes_bool_markers_and_produces_ordinary_ints() {
        let cases = [
            (int_add(BOOL_TRUE_MARKER, tag_smallint(1)), 2),
            (int_add(tag_smallint(1), BOOL_TRUE_MARKER), 2),
            (int_add(BOOL_FALSE_MARKER, tag_smallint(2)), 2),
            (int_add(tag_smallint(2), BOOL_FALSE_MARKER), 2),
            (int_sub(BOOL_TRUE_MARKER, tag_smallint(1)), 0),
            (int_sub(tag_smallint(1), BOOL_TRUE_MARKER), 0),
            (int_mul(BOOL_TRUE_MARKER, tag_smallint(2)), 2),
            (int_mul(tag_smallint(2), BOOL_TRUE_MARKER), 2),
            (int_mul(BOOL_FALSE_MARKER, tag_smallint(2)), 0),
            (int_mul(tag_smallint(2), BOOL_FALSE_MARKER), 0),
            (int_floordiv(BOOL_FALSE_MARKER, tag_smallint(2)), 0),
            (int_floordiv(tag_smallint(2), BOOL_TRUE_MARKER), 2),
            (int_floormod(BOOL_FALSE_MARKER, tag_smallint(2)), 0),
            (int_floormod(tag_smallint(2), BOOL_TRUE_MARKER), 0),
            (int_pow(BOOL_FALSE_MARKER, BOOL_TRUE_MARKER), 0),
            (int_pow(tag_smallint(2), BOOL_TRUE_MARKER), 2),
            (int_pow(BOOL_TRUE_MARKER, tag_smallint(2)), 1),
            (int_pow(tag_smallint(2), BOOL_FALSE_MARKER), 1),
        ];
        for (actual, expected) in cases {
            assert_eq!(actual, tag_smallint(expected));
            assert!(is_smallint(actual));
        }

        assert_eq!(int_cmp(BOOL_FALSE_MARKER, tag_smallint(0)), 0);
        assert_eq!(int_cmp(tag_smallint(1), BOOL_TRUE_MARKER), 0);
        assert_eq!(int_cmp(BOOL_FALSE_MARKER, BOOL_TRUE_MARKER), -1);
        assert_eq!(
            range_continue(BOOL_FALSE_MARKER, BOOL_TRUE_MARKER, BOOL_TRUE_MARKER),
            1
        );
        assert_eq!(
            range_continue(BOOL_TRUE_MARKER, BOOL_TRUE_MARKER, BOOL_TRUE_MARKER),
            0
        );
    }

    #[test]
    #[should_panic(expected = "pycc_rt: range() arg 3 must not be zero")]
    fn false_identity_marker_is_a_zero_range_step() {
        range_continue(tag_smallint(0), tag_smallint(1), BOOL_FALSE_MARKER);
    }

    #[test]
    fn add_and_sub_accept_bool_markers_next_to_bigints_in_both_positions() {
        fn assert_value(encoded: i64, expected: i128) {
            let expected = bigint_from_i128(expected);
            let (actual_negative, actual_limbs) = to_sign_and_magnitude(encoded);
            assert_eq!(actual_negative, expected.negative);
            assert_eq!(actual_limbs, expected.limbs);
        }

        let magnitude = 1i128 << 80;
        let bigint = tag_bigint(bigint_from_i128(magnitude));
        assert_value(int_add(bigint, BOOL_TRUE_MARKER), magnitude + 1);
        assert_value(int_add(BOOL_TRUE_MARKER, bigint), magnitude + 1);
        assert_value(int_sub(bigint, BOOL_TRUE_MARKER), magnitude - 1);
        assert_value(int_sub(BOOL_TRUE_MARKER, bigint), 1 - magnitude);
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
    fn int_untag_checked_recovers_the_original_value() {
        // Calls the public `extern "C"` wrapper (not the private
        // `int_untag_checked`) on purpose: this is the non-panicking path,
        // so no unwind ever crosses that boundary, and it is what covers
        // the wrapper's own line -- the same split this file's `int_add`
        // implementation note establishes for every panicking
        // `pycc_rt_int_*` symbol.
        for n in [0i64, 1, -1, 42, -42, i64::MAX >> 1, -(i64::MAX >> 1)] {
            assert_eq!(pycc_rt_int_untag_checked(tag_smallint(n)), n);
        }
    }

    #[test]
    #[should_panic(expected = "pycc_rt: int boundary does not support bigint-valued values yet")]
    fn int_untag_checked_rejects_a_bigint_tagged_value() {
        // A bigint-tagged value has its low bit clear (see TAG_BIT/
        // is_smallint). `tag_bigint` itself takes a `BigIntObj`, not a bare
        // i64 -- rather than hand-craft a bigint-tagged bit pattern from
        // scratch, force a real overflow through the existing arithmetic
        // path, exactly like this file's own `repeated_addition_exercises_
        // the_general_bigint_plus_smallint_path` test already does:
        // `i64::MAX >> 1` is exactly the largest value that still
        // round-trips through tagging, so adding it to itself overflows and
        // promotes via `int_add`'s own `tag_bigint` path.
        //
        // The setup call uses the public `pycc_rt_int_add` wrapper (that
        // path does not panic); the assertion below calls the *private*
        // `int_untag_checked`, because a panic unwinding past a plain
        // `extern "C" fn`'s boundary aborts the whole test binary instead
        // of being caught by `#[should_panic]` -- see the implementation
        // note above `int_add`.
        let bigint_tagged =
            pycc_rt_int_add(tag_smallint(i64::MAX >> 1), tag_smallint(i64::MAX >> 1));
        assert!(
            !is_smallint(bigint_tagged),
            "test setup: expected this addition to overflow into a real bigint"
        );
        int_untag_checked(bigint_tagged);
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
    fn pycc_rt_float_pow_accepts_a_negative_base_with_an_integer_exponent() {
        // An integer exponent (positive or negative) always yields a real
        // result, matching `python3.13`: `(-2.0) ** 3 == -8.0`,
        // `(-2.0) ** -3 == -0.125`, `(-2.0) ** 2 == 4.0`.
        assert_eq!(pycc_rt_float_pow(-2.0, 3.0), -8.0);
        assert_eq!(pycc_rt_float_pow(-2.0, -3.0), -0.125);
        assert_eq!(pycc_rt_float_pow(-2.0, 2.0), 4.0);
    }

    #[test]
    fn pycc_rt_float_pow_accepts_a_tiny_finite_result() {
        // Verified against `python3.13`: `2.0 ** -1024.0` produces a tiny
        // finite positive value, no exception -- only *overflow* to infinity
        // is a Python `OverflowError`, not a representable subnormal result.
        let result = pycc_rt_float_pow(2.0, -1024.0);
        assert!(result > 0.0 && result < 1e-300);
    }

    #[test]
    #[should_panic(expected = "0.0 cannot be raised to a negative power")]
    fn float_pow_rejects_zero_raised_to_a_negative_power() {
        // Verified against `python3.13`: `0.0 ** -1.0` raises
        // `ZeroDivisionError: 0.0 cannot be raised to a negative power`,
        // while `f64::powf` alone silently returns `inf`. Calls the
        // private `float_pow` directly, not the public `extern "C"`
        // wrapper -- the wrapper is a plain (non-unwinding) `extern "C" fn`,
        // so a panic crossing it aborts the whole test binary (`SIGABRT`)
        // instead of being caught by `#[should_panic]` (same convention as
        // `int_mul`/`float_div` above).
        float_pow(0.0, -1.0);
    }

    #[test]
    #[should_panic(expected = "0.0 cannot be raised to a negative power")]
    fn float_pow_rejects_negative_zero_raised_to_a_negative_power() {
        // `-0.0 == 0.0` under IEEE-754, and `python3.13` raises the same
        // `ZeroDivisionError` for `(-0.0) ** -1.0` as for `0.0 ** -1.0`.
        float_pow(-0.0, -1.0);
    }

    #[test]
    #[should_panic(
        expected = "a negative float raised to a non-integer power is not supported yet"
    )]
    fn float_pow_rejects_a_negative_base_raised_to_a_non_integer_power() {
        // Verified against `python3.13`: `(-2.0) ** 3.5` returns a complex
        // number (`pycc` has no complex type in v0.1), while `f64::powf`
        // alone silently returns `NaN`.
        float_pow(-2.0, 3.5);
    }

    #[test]
    #[should_panic(expected = "float power overflowed")]
    fn float_pow_rejects_a_finite_result_that_overflows_float_range() {
        // Verified against `python3.13`: `2.0 ** 1024.0` raises
        // `OverflowError: (34, 'Result too large')`, while `f64::powf`
        // alone silently returns `inf`.
        float_pow(2.0, 1024.0);
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

    #[test]
    fn int_list_new_starts_empty() {
        // Every `pycc_rt_int_list_*` function taking/dereferencing a
        // `*mut PyIntListObj` is `unsafe extern "C"` (see their own doc
        // comments' `# Safety` sections), matching `PyStrObj`'s own
        // convention -- so, same as `a_short_literal_round_trips_through_
        // the_inline_representation` above, this wraps the whole body in
        // one `unsafe { ... }` block rather than the task brief's own
        // unwrapped calls (a genuine compile-error bug in the brief, not
        // a change to what the test asserts).
        unsafe {
            let list = pycc_rt_int_list_new();
            assert_eq!(pycc_rt_int_list_len(list), 0);
            pycc_rt_int_list_decref(list);
        }
    }

    #[test]
    fn int_list_append_then_get_round_trips() {
        unsafe {
            let list = pycc_rt_int_list_new();
            pycc_rt_int_list_append(list, tag_smallint(10));
            pycc_rt_int_list_append(list, tag_smallint(20));
            pycc_rt_int_list_append(list, tag_smallint(30));
            assert_eq!(pycc_rt_int_list_len(list), 3);
            assert_eq!(pycc_rt_int_list_get(list, 0), tag_smallint(10));
            assert_eq!(pycc_rt_int_list_get(list, 1), tag_smallint(20));
            assert_eq!(pycc_rt_int_list_get(list, 2), tag_smallint(30));
            pycc_rt_int_list_decref(list);
        }
    }

    #[test]
    fn int_list_read_slice_and_pop_preserve_bool_identity_markers() {
        unsafe {
            let list = pycc_rt_int_list_new();
            pycc_rt_int_list_append(list, BOOL_FALSE_MARKER);
            pycc_rt_int_list_append(list, BOOL_TRUE_MARKER);
            assert_eq!(pycc_rt_int_list_get(list, 0), BOOL_FALSE_MARKER);
            assert_eq!(pycc_rt_int_list_get(list, 1), BOOL_TRUE_MARKER);

            let sliced = pycc_rt_int_list_slice(list, 0, 2, 1);
            assert_eq!(pycc_rt_int_list_get(sliced, 0), BOOL_FALSE_MARKER);
            assert_eq!(pycc_rt_int_list_get(sliced, 1), BOOL_TRUE_MARKER);
            assert_eq!(pycc_rt_int_list_pop(list), BOOL_TRUE_MARKER);
            pycc_rt_int_list_decref(sliced);
            pycc_rt_int_list_decref(list);
        }
    }

    #[test]
    fn int_list_grows_past_its_initial_capacity() {
        unsafe {
            let list = pycc_rt_int_list_new();
            for i in 0..1000 {
                pycc_rt_int_list_append(list, tag_smallint(i));
            }
            assert_eq!(pycc_rt_int_list_len(list), 1000);
            assert_eq!(pycc_rt_int_list_get(list, 999), tag_smallint(999));
            pycc_rt_int_list_decref(list);
        }
    }

    #[test]
    #[should_panic(expected = "pycc_rt: list index out of range")]
    fn int_list_get_out_of_range_panics_honestly() {
        // Deviation from the task brief: calls the private `int_list_get`
        // directly, not the public `pycc_rt_int_list_get` wrapper the
        // brief used -- the wrapper is a plain `unsafe extern "C" fn`, so
        // a panic crossing its boundary aborts the whole test binary
        // (`SIGABRT`) instead of unwinding into `#[should_panic]`'s own
        // catch (see the private-logic/public-wrapper split added above,
        // same convention as `int_add`/`pycc_rt_int_add` and
        // `float_to_str`/`pycc_rt_float_to_str`).
        unsafe {
            let list = pycc_rt_int_list_new();
            pycc_rt_int_list_append(list, tag_smallint(1));
            int_list_get(&*list, 5);
        }
    }

    #[test]
    #[should_panic(expected = "pycc_rt: list index out of range")]
    fn int_list_get_rejects_negative_indices() {
        // D-108's documented v0.2 scope cut: unlike real Python (where
        // `lst[-1]` is the last element), a negative index panics here
        // exactly like an out-of-range positive one -- `pycc_types` has no
        // way to reject a negative index at compile time, so this is
        // pycc's actual, observable runtime behavior for `lst[-1]` today.
        // Not something Task 12's PEP-585 conformance fixture may exercise
        // (it would panic instead of byte-for-byte matching CPython).
        unsafe {
            let list = pycc_rt_int_list_new();
            pycc_rt_int_list_append(list, tag_smallint(1));
            int_list_get(&*list, -1);
        }
    }

    #[test]
    fn int_list_incref_decref_round_trip_does_not_free_early() {
        unsafe {
            let list = pycc_rt_int_list_new();
            pycc_rt_int_list_append(list, tag_smallint(1));
            pycc_rt_int_list_incref(list);
            pycc_rt_int_list_decref(list);
            // still alive after one incref/decref pair -- one decref remains
            assert_eq!(pycc_rt_int_list_get(list, 0), tag_smallint(1));
            pycc_rt_int_list_decref(list);
        }
    }

    #[test]
    fn int_list_incref_and_decref_on_a_null_pointer_are_safe_no_ops() {
        // D-014's 100% line/region coverage gate: without this,
        // `pycc_rt_int_list_incref`/`_decref`'s `if list.is_null()` early
        // return is dead code, since none of the tests above ever pass a
        // null pointer. Mirrors `PyStrObj`'s own
        // `incref_and_decref_on_a_null_pointer_are_safe_no_ops` test above.
        unsafe {
            pycc_rt_int_list_incref(std::ptr::null_mut());
            pycc_rt_int_list_decref(std::ptr::null_mut());
        }
    }

    /// Reads every encoded element of `list` (possibly zero, for an empty
    /// slice result) into a `Vec<i64>`, for asserting a whole slice result's
    /// contents in one line rather than one `pycc_rt_int_list_get` call per
    /// expected element.
    unsafe fn collect_list(list: *mut PyIntListObj) -> Vec<i64> {
        unsafe {
            let len = pycc_rt_int_list_len(list);
            (0..len).map(|i| pycc_rt_int_list_get(list, i)).collect()
        }
    }

    #[test]
    fn pycc_rt_int_list_slice_returns_the_in_range_sub_range() {
        // D-118's ordinary path: `[10,20,30,40,50][1:4:1] == [20,30,40]`.
        unsafe {
            let list = pycc_rt_int_list_new();
            for v in [10, 20, 30, 40, 50] {
                pycc_rt_int_list_append(list, tag_smallint(v));
            }
            let sliced = pycc_rt_int_list_slice(list, 1, 4, 1);
            assert_eq!(
                collect_list(sliced),
                vec![tag_smallint(20), tag_smallint(30), tag_smallint(40)]
            );
            pycc_rt_int_list_decref(list);
            pycc_rt_int_list_decref(sliced);
        }
    }

    #[test]
    fn pycc_rt_int_list_slice_clamps_a_too_high_stop() {
        // D-118's clamp-stop-high branch: `[1,2,3][0:100:1] == [1,2,3]`,
        // matching CPython's own out-of-range-slice-bound clamping.
        unsafe {
            let list = pycc_rt_int_list_new();
            for v in [1, 2, 3] {
                pycc_rt_int_list_append(list, tag_smallint(v));
            }
            let sliced = pycc_rt_int_list_slice(list, 0, 100, 1);
            assert_eq!(
                collect_list(sliced),
                vec![tag_smallint(1), tag_smallint(2), tag_smallint(3)]
            );
            pycc_rt_int_list_decref(list);
            pycc_rt_int_list_decref(sliced);
        }
    }

    #[test]
    fn pycc_rt_int_list_slice_clamps_a_too_high_start() {
        // D-118's clamp-start-high branch: `[1,2,3][100:200:1]` clamps
        // *both* bounds to `len` (3), so `clamped_start == clamped_stop`
        // and the result is empty -- the same outcome as the
        // start-at-or-past-stop test below, but pinning the clamp
        // arithmetic itself (both operands clamp to the same value) rather
        // than an already-in-range `start >= stop` relationship.
        unsafe {
            let list = pycc_rt_int_list_new();
            for v in [1, 2, 3] {
                pycc_rt_int_list_append(list, tag_smallint(v));
            }
            let sliced = pycc_rt_int_list_slice(list, 100, 200, 1);
            assert_eq!(collect_list(sliced), Vec::<i64>::new());
            pycc_rt_int_list_decref(list);
            pycc_rt_int_list_decref(sliced);
        }
    }

    #[test]
    fn pycc_rt_int_list_slice_with_start_at_or_past_stop_is_empty() {
        // D-118's empty-result branch: `[1,2,3][2:1:1] == []` -- `start`
        // and `stop` are both already in range, unlike the clamp-driven
        // empty result above, so the `while i < clamped_stop` loop body
        // never runs for a distinct reason (no clamping involved at all).
        unsafe {
            let list = pycc_rt_int_list_new();
            for v in [1, 2, 3] {
                pycc_rt_int_list_append(list, tag_smallint(v));
            }
            let sliced = pycc_rt_int_list_slice(list, 2, 1, 1);
            assert_eq!(collect_list(sliced), Vec::<i64>::new());
            pycc_rt_int_list_decref(list);
            pycc_rt_int_list_decref(sliced);
        }
    }

    #[test]
    #[should_panic(expected = "pycc_rt: slice start must be non-negative")]
    fn pycc_rt_int_list_slice_rejects_negative_start() {
        // Calls the private `int_list_slice` directly, not the public
        // `pycc_rt_int_list_slice` wrapper -- the wrapper is a plain
        // `unsafe extern "C" fn`, so a panic crossing its boundary aborts
        // the whole test binary instead of unwinding into
        // `#[should_panic]`'s own catch, exactly like
        // `int_list_get_out_of_range_panics_honestly`'s own established
        // convention above.
        unsafe {
            let list = pycc_rt_int_list_new();
            pycc_rt_int_list_append(list, tag_smallint(1));
            int_list_slice(&*list, -1, 1, 1);
        }
    }

    #[test]
    #[should_panic(expected = "pycc_rt: slice stop must be non-negative")]
    fn pycc_rt_int_list_slice_rejects_negative_stop() {
        unsafe {
            let list = pycc_rt_int_list_new();
            pycc_rt_int_list_append(list, tag_smallint(1));
            int_list_slice(&*list, 0, -1, 1);
        }
    }

    #[test]
    #[should_panic(expected = "pycc_rt: slice step must be positive")]
    fn pycc_rt_int_list_slice_rejects_zero_step() {
        unsafe {
            let list = pycc_rt_int_list_new();
            pycc_rt_int_list_append(list, tag_smallint(1));
            int_list_slice(&*list, 0, 1, 0);
        }
    }

    #[test]
    #[should_panic(expected = "pycc_rt: slice step must be positive")]
    fn pycc_rt_int_list_slice_rejects_negative_step() {
        unsafe {
            let list = pycc_rt_int_list_new();
            pycc_rt_int_list_append(list, tag_smallint(1));
            int_list_slice(&*list, 0, 1, -1);
        }
    }

    #[test]
    fn pycc_rt_int_list_slice_with_a_step_greater_than_one_skips_elements() {
        // `[0,1,2,3,4,5][0:6:2] == [0,2,4]`.
        unsafe {
            let list = pycc_rt_int_list_new();
            for v in [0, 1, 2, 3, 4, 5] {
                pycc_rt_int_list_append(list, tag_smallint(v));
            }
            let sliced = pycc_rt_int_list_slice(list, 0, 6, 2);
            assert_eq!(
                collect_list(sliced),
                vec![tag_smallint(0), tag_smallint(2), tag_smallint(4)]
            );
            pycc_rt_int_list_decref(list);
            pycc_rt_int_list_decref(sliced);
        }
    }

    #[test]
    fn pycc_rt_int_list_slice_returns_a_genuinely_independent_list() {
        // D-107's leak-only policy still requires the slice result to be a
        // *new* allocation, not an alias of `list`'s own backing storage --
        // appending to the original after slicing must not retroactively
        // change the already-returned slice, and vice versa.
        unsafe {
            let list = pycc_rt_int_list_new();
            for v in [1, 2, 3] {
                pycc_rt_int_list_append(list, tag_smallint(v));
            }
            let sliced = pycc_rt_int_list_slice(list, 0, 3, 1);
            pycc_rt_int_list_append(list, tag_smallint(99));
            assert_eq!(
                collect_list(list),
                vec![
                    tag_smallint(1),
                    tag_smallint(2),
                    tag_smallint(3),
                    tag_smallint(99)
                ]
            );
            assert_eq!(
                collect_list(sliced),
                vec![tag_smallint(1), tag_smallint(2), tag_smallint(3)]
            );
            pycc_rt_int_list_decref(list);
            pycc_rt_int_list_decref(sliced);
        }
    }

    #[test]
    fn pycc_rt_int_list_pop_removes_and_returns_the_last_element() {
        // Python's `list.pop()`: `[1,2,3].pop() == 3`, and the list shrinks
        // by one, leaving `[1,2]` -- the removed element is always the
        // *last* one, D-119.
        unsafe {
            let list = pycc_rt_int_list_new();
            for v in [1, 2, 3] {
                pycc_rt_int_list_append(list, tag_smallint(v));
            }
            assert_eq!(pycc_rt_int_list_pop(list), tag_smallint(3));
            assert_eq!(pycc_rt_int_list_len(list), 2);
            assert_eq!(collect_list(list), vec![tag_smallint(1), tag_smallint(2)]);
            pycc_rt_int_list_decref(list);
        }
    }

    #[test]
    fn pycc_rt_int_list_pop_repeated_calls_keep_removing_the_new_last_element() {
        // Two `.pop()`s in a row must each observe the *previous* pop's
        // effect, not some stale snapshot -- exercises the same repeated-
        // mutation-on-the-same-object concern this task's own brief flags
        // for `xs = [xs.pop(), xs.pop()]`-shaped codegen, at the `pycc_rt`
        // layer directly.
        unsafe {
            let list = pycc_rt_int_list_new();
            for v in [1, 2, 3] {
                pycc_rt_int_list_append(list, tag_smallint(v));
            }
            assert_eq!(pycc_rt_int_list_pop(list), tag_smallint(3));
            assert_eq!(pycc_rt_int_list_pop(list), tag_smallint(2));
            assert_eq!(pycc_rt_int_list_len(list), 1);
            assert_eq!(collect_list(list), vec![tag_smallint(1)]);
            pycc_rt_int_list_decref(list);
        }
    }

    #[test]
    #[should_panic(expected = "pycc_rt: pop from empty list")]
    fn pycc_rt_int_list_pop_on_an_empty_list_panics_honestly() {
        // Calls the private `int_list_pop` directly, not the public
        // `pycc_rt_int_list_pop` wrapper -- the wrapper is a plain `unsafe
        // extern "C" fn`, so a panic crossing its boundary aborts the whole
        // test binary instead of unwinding into `#[should_panic]`'s own
        // catch, exactly like `int_list_get_out_of_range_panics_honestly`'s
        // own established convention above.
        unsafe {
            let list = pycc_rt_int_list_new();
            int_list_pop(&*list);
        }
    }

    #[test]
    fn pycc_rt_dict_set_then_get_round_trips_the_value() {
        unsafe {
            let dict = pycc_rt_dict_new();
            let key = new_pystr(b"a");
            pycc_rt_dict_set(dict, key, tag_smallint(42));
            assert_eq!(pycc_rt_dict_get(dict, key), tag_smallint(42));
            assert_eq!(pycc_rt_dict_len(dict), 1);
            pycc_rt_dict_decref(dict);
        }
    }

    #[test]
    fn dict_update_get_and_default_preserve_bool_identity_markers() {
        unsafe {
            let dict = pycc_rt_dict_new();
            let present = new_pystr(b"present");
            let missing = new_pystr(b"missing");
            pycc_rt_dict_set(dict, present, BOOL_TRUE_MARKER);
            assert_eq!(pycc_rt_dict_get(dict, present), BOOL_TRUE_MARKER);
            assert_eq!(
                pycc_rt_dict_get_or_default(dict, present, BOOL_FALSE_MARKER),
                BOOL_TRUE_MARKER
            );
            assert_eq!(
                pycc_rt_dict_get_or_default(dict, missing, BOOL_FALSE_MARKER),
                BOOL_FALSE_MARKER
            );
            pycc_rt_dict_decref(dict);
        }
    }

    #[test]
    fn pycc_rt_dict_set_on_an_existing_key_updates_in_place_without_growing_len() {
        unsafe {
            let dict = pycc_rt_dict_new();
            let key = new_pystr(b"a");
            pycc_rt_dict_set(dict, key, tag_smallint(1));
            pycc_rt_dict_set(dict, key, tag_smallint(2));
            assert_eq!(pycc_rt_dict_get(dict, key), tag_smallint(2));
            assert_eq!(pycc_rt_dict_len(dict), 1);
            pycc_rt_dict_decref(dict);
        }
    }

    #[test]
    fn pycc_rt_dict_preserves_insertion_order_across_key_at() {
        unsafe {
            let dict = pycc_rt_dict_new();
            let a = new_pystr(b"a");
            let b = new_pystr(b"b");
            pycc_rt_dict_set(dict, b, tag_smallint(2));
            pycc_rt_dict_set(dict, a, tag_smallint(1));
            // "b" was inserted first, so it stays at index 0 even though "a"
            // sorts first lexicographically.
            assert_eq!(pycc_rt_str_cmp(pycc_rt_dict_key_at(dict, 0), b), 0);
            assert_eq!(pycc_rt_str_cmp(pycc_rt_dict_key_at(dict, 1), a), 0);
            pycc_rt_dict_decref(dict);
        }
    }

    #[test]
    #[should_panic(expected = "pycc_rt: dict_key_at index out of range")]
    fn pycc_rt_dict_key_at_out_of_range_panics_honestly() {
        unsafe {
            let dict = pycc_rt_dict_new();
            let key = new_pystr(b"a");
            pycc_rt_dict_set(dict, key, tag_smallint(1));
            // Calls the private `dict_key_at`, not the public
            // `pycc_rt_dict_key_at` wrapper -- the wrapper is a plain
            // `extern "C" fn`, so a panic crossing its boundary aborts the
            // whole test binary (`SIGABRT`) instead of unwinding into
            // `#[should_panic]`'s own catch (same convention as
            // `int_list_get_out_of_range_panics_honestly`/
            // `pycc_rt_dict_get_on_a_missing_key_panics` just below).
            dict_key_at(&*dict, 1);
        }
    }

    #[test]
    #[should_panic(expected = "pycc_rt: dict key not found")]
    fn pycc_rt_dict_get_on_a_missing_key_panics() {
        unsafe {
            let dict = pycc_rt_dict_new();
            let key = new_pystr(b"missing");
            // Calls the private `dict_get`, not the public `pycc_rt_dict_get` wrapper
            // -- the wrapper is a plain `extern "C" fn`, so a panic crossing its
            // boundary aborts the whole test binary (`SIGABRT`) instead of unwinding
            // into `#[should_panic]`'s own catch (see the private-logic/public-wrapper
            // split added above, same convention as `int_list_get_out_of_range_panics_honestly`).
            dict_get(&*dict, key);
        }
    }

    #[test]
    fn pycc_rt_dict_get_or_default_on_a_present_key_returns_the_stored_value() {
        // Python's `dict.get(key, default)` on a present key: the stored
        // value wins, `default` is ignored -- the two-argument form's
        // "found" path, D-119.
        unsafe {
            let dict = pycc_rt_dict_new();
            let key = new_pystr(b"a");
            pycc_rt_dict_set(dict, key, tag_smallint(42));
            assert_eq!(
                pycc_rt_dict_get_or_default(dict, key, tag_smallint(-1)),
                tag_smallint(42)
            );
            pycc_rt_dict_decref(dict);
        }
    }

    #[test]
    fn pycc_rt_dict_get_or_default_on_a_missing_key_returns_the_default_without_panicking() {
        // Unlike `pycc_rt_dict_get`, a missing key never panics here -- it
        // returns `default` instead, the entire point of the two-argument
        // form, D-119.
        unsafe {
            let dict = pycc_rt_dict_new();
            let key = new_pystr(b"a");
            pycc_rt_dict_set(dict, key, tag_smallint(42));
            let missing = new_pystr(b"missing");
            assert_eq!(
                pycc_rt_dict_get_or_default(dict, missing, tag_smallint(-1)),
                tag_smallint(-1)
            );
            pycc_rt_dict_decref(dict);
        }
    }

    #[test]
    fn pycc_rt_dict_get_or_default_on_an_empty_dict_returns_the_default() {
        // No entries at all -- the degenerate case of the missing-key path
        // above, pinned separately since `dict_get_or_default`'s own linear
        // scan over an empty `Vec` is a distinct code path worth its own
        // executing test.
        unsafe {
            let dict = pycc_rt_dict_new();
            let key = new_pystr(b"z");
            assert_eq!(
                pycc_rt_dict_get_or_default(dict, key, tag_smallint(7)),
                tag_smallint(7)
            );
            pycc_rt_dict_decref(dict);
        }
    }

    #[test]
    fn pycc_rt_dict_incref_then_decref_frees_without_leaking() {
        unsafe {
            let dict = pycc_rt_dict_new();
            pycc_rt_dict_incref(dict);
            pycc_rt_dict_decref(dict);
            pycc_rt_dict_decref(dict); // rc reaches 0, frees
        }
    }

    #[test]
    fn pycc_rt_dict_incref_and_decref_on_a_null_pointer_are_safe_no_ops() {
        // D-014's 100% line/region coverage gate: without this,
        // `pycc_rt_dict_incref`/`_decref`'s `if dict.is_null()` early
        // return is dead code, since none of the tests above ever pass a
        // null pointer. Mirrors `PyIntListObj`'s own
        // `int_list_incref_and_decref_on_a_null_pointer_are_safe_no_ops`
        // test above.
        unsafe {
            pycc_rt_dict_incref(std::ptr::null_mut());
            pycc_rt_dict_decref(std::ptr::null_mut());
        }
    }

    #[test]
    fn pycc_rt_int_set_check_not_resized_is_a_no_op_when_lengths_match() {
        // Calls the public wrapper directly (safe for the non-panicking
        // path, unlike the panic-path test below), so the wrapper's own
        // call-through line is exercised too, not just the private helper.
        pycc_rt_int_set_check_not_resized(3, 3);
    }

    #[test]
    #[should_panic(expected = "pycc_rt: set changed size during iteration")]
    fn check_set_len_unchanged_panics_when_lengths_differ() {
        // Calls the private `check_set_len_unchanged` directly, not the
        // public `pycc_rt_int_set_check_not_resized` wrapper -- the wrapper
        // is a plain `extern "C" fn`, so a panic crossing its boundary
        // aborts the whole test binary instead of unwinding into
        // `#[should_panic]`'s own catch, exactly like
        // `pycc_rt_int_list_pop_on_an_empty_list_panics_honestly`'s own
        // established convention above.
        check_set_len_unchanged(4, 3);
    }

    #[test]
    fn pycc_rt_int_set_add_deduplicates_repeated_values() {
        unsafe {
            let set = pycc_rt_int_set_new();
            pycc_rt_int_set_add(set, tag_smallint(1));
            pycc_rt_int_set_add(set, tag_smallint(1));
            pycc_rt_int_set_add(set, tag_smallint(2));
            assert_eq!(pycc_rt_int_set_len(set), 2);
            pycc_rt_int_set_decref(set);
        }
    }

    #[test]
    fn pycc_rt_int_set_preserves_first_insertion_order() {
        unsafe {
            let set = pycc_rt_int_set_new();
            pycc_rt_int_set_add(set, tag_smallint(2));
            pycc_rt_int_set_add(set, tag_smallint(1));
            pycc_rt_int_set_add(set, tag_smallint(2)); // duplicate, ignored, does not move 2's position
            assert_eq!(pycc_rt_int_set_get(set, 0), tag_smallint(2));
            assert_eq!(pycc_rt_int_set_get(set, 1), tag_smallint(1));
            pycc_rt_int_set_decref(set);
        }
    }

    #[test]
    fn int_set_numeric_dedup_preserves_the_first_bool_or_int_encoding() {
        unsafe {
            let bool_first = pycc_rt_int_set_new();
            pycc_rt_int_set_add(bool_first, BOOL_TRUE_MARKER);
            pycc_rt_int_set_add(bool_first, tag_smallint(1));
            assert_eq!(pycc_rt_int_set_len(bool_first), 1);
            assert_eq!(pycc_rt_int_set_get(bool_first, 0), BOOL_TRUE_MARKER);
            pycc_rt_int_set_decref(bool_first);

            let int_first = pycc_rt_int_set_new();
            pycc_rt_int_set_add(int_first, tag_smallint(0));
            pycc_rt_int_set_add(int_first, BOOL_FALSE_MARKER);
            assert_eq!(pycc_rt_int_set_len(int_first), 1);
            assert_eq!(pycc_rt_int_set_get(int_first, 0), tag_smallint(0));
            pycc_rt_int_set_decref(int_first);
        }
    }

    #[test]
    fn pycc_rt_int_set_incref_then_decref_frees_without_leaking() {
        unsafe {
            let set = pycc_rt_int_set_new();
            pycc_rt_int_set_incref(set);
            pycc_rt_int_set_decref(set);
            pycc_rt_int_set_decref(set);
        }
    }

    #[test]
    fn pycc_rt_int_set_incref_and_decref_on_a_null_pointer_are_safe_no_ops() {
        // D-014's 100% line/region coverage gate: without this,
        // `pycc_rt_int_set_incref`/`_decref`'s `if set.is_null()` early
        // return is dead code, since none of the tests above ever pass a
        // null pointer. Mirrors `PyIntListObj`'s own
        // `int_list_incref_and_decref_on_a_null_pointer_are_safe_no_ops`
        // test above.
        unsafe {
            pycc_rt_int_set_incref(std::ptr::null_mut());
            pycc_rt_int_set_decref(std::ptr::null_mut());
        }
    }
}
