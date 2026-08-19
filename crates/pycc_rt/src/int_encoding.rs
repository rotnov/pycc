//! `int`'s one-word encoding and its heap bigint representation.
//!
//! Split out of `lib.rs` under this repository's "keep source files
//! decomposable" rule while #147 (D-179) was adding `magnitude_sign` and
//! `encoded_int_cmp` here. The cut is by cohesion: everything that decides
//! *how an `int` value is represented* lives in this module, while the
//! arithmetic, comparison, and formatting operations *over* that
//! representation stay in `lib.rs` beside their `extern "C"` wrappers. No
//! `#[unsafe(no_mangle)] pub extern "C"` symbol moved -- the ABI surface is
//! unchanged.
//!
//! Everything here is `pub(crate)`: `pycc_rt` owns this representation, and
//! nothing outside the crate may interpret an encoded word.

/// See D-061/D-141: every int-compatible value is one LLVM `i64`. Odd words
/// are ordinary smallints; exact words `2` and `6` preserve `False` and
/// `True` identity after a bool-to-int boundary; non-zero words aligned to
/// four bytes are heap `BigInt` pointers. `classify_encoded_int` is the one
/// fail-closed classifier used before interpreting any even word.
pub(crate) const TAG_BIT: i64 = 1;
pub(crate) const LOW_TAG_MASK: i64 = 0b11;
pub(crate) const BOOL_FALSE_MARKER: i64 = 0b0010;
pub(crate) const BOOL_TRUE_MARKER: i64 = 0b0110;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EncodedIntKind {
    SmallInt,
    BoolFalse,
    BoolTrue,
    BigInt,
}

pub(crate) fn tag_smallint(value: i64) -> i64 {
    (value << 1) | TAG_BIT
}

pub(crate) fn untag_smallint(tagged: i64) -> i64 {
    tagged >> 1 // arithmetic (sign-extending) shift for `i64`
}

pub(crate) fn is_smallint(tagged: i64) -> bool {
    tagged & TAG_BIT == TAG_BIT
}

/// Classifies every word in the int-compatible ABI before any pointer cast.
/// Odd words are ordinary smallints; two exact low-tag-`10` words preserve
/// bool identity; non-zero aligned words are bigint pointers. Every other
/// pattern fails closed instead of being dereferenced as an attacker-chosen
/// pointer.
pub(crate) fn classify_encoded_int(encoded: i64) -> EncodedIntKind {
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

pub(crate) fn inline_int_value(encoded: i64) -> Option<i64> {
    match classify_encoded_int(encoded) {
        EncodedIntKind::SmallInt => Some(untag_smallint(encoded)),
        EncodedIntKind::BoolFalse => Some(0),
        EncodedIntKind::BoolTrue => Some(1),
        EncodedIntKind::BigInt => None,
    }
}

/// `None` when `value` needs the full 64 bits (including sign) to
/// represent -- i.e. tagging then untagging would not round-trip.
pub(crate) fn fits_smallint(value: i64) -> Option<i64> {
    let tagged = tag_smallint(value);
    (untag_smallint(tagged) == value).then_some(tagged)
}

pub(crate) fn require_inline_int(encoded: i64, context: &str) -> i64 {
    inline_int_value(encoded)
        .unwrap_or_else(|| panic!("pycc_rt: {context} a bigint-valued `int` is not supported yet"))
}

/// D-058: hand-rolled sign-magnitude limbs, base 2^32, little-endian,
/// no trailing zero limbs except a single `[0]` representing zero
/// itself. Never freed (leaked) -- unlike `PyStrObj`, D-060 only commits
/// `str` to real refcounting; a bigint is an overflow-only path, and the
/// concession is a deliberate, narrower "simplest safe default" than
/// `str`'s, recorded alongside D-061. Since #147 (D-179) a `range` loop
/// whose induction variable crosses the tagged range keeps iterating
/// instead of aborting, so each such iteration's `int_add` leaks one
/// `BigIntObj`; that is now the widest exposure of this concession, and it
/// is bounded by the loop's own trip count rather than being unbounded.
pub(crate) struct BigIntObj {
    pub(crate) negative: bool,
    pub(crate) limbs: Vec<u32>,
}

const _: () = assert!(std::mem::align_of::<BigIntObj>() >= 4);
const _: () = assert!(std::mem::size_of::<*const BigIntObj>() <= std::mem::size_of::<i64>());

pub(crate) fn trim(limbs: &[u32]) -> Vec<u32> {
    let mut end = limbs.len();
    while end > 1 && limbs[end - 1] == 0 {
        end -= 1;
    }
    limbs[..end].to_vec()
}

pub(crate) fn magnitude_cmp(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
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

pub(crate) fn magnitude_add(a: &[u32], b: &[u32]) -> Vec<u32> {
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
pub(crate) fn magnitude_sub(a: &[u32], b: &[u32]) -> Vec<u32> {
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

pub(crate) fn bigint_add_signed(
    a_neg: bool,
    a_mag: &[u32],
    b_neg: bool,
    b_mag: &[u32],
) -> BigIntObj {
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

pub(crate) fn bigint_from_i128(v: i128) -> BigIntObj {
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

pub(crate) fn tag_bigint(b: BigIntObj) -> i64 {
    Box::into_raw(Box::new(b)) as i64
}

/// # Safety
/// `tagged` must classify as a `BigIntObj` pointer. Classification is
/// repeated here so no dereference can bypass the fail-closed tag check.
pub(crate) unsafe fn bigint_ref<'a>(tagged: i64) -> &'a BigIntObj {
    if classify_encoded_int(tagged) != EncodedIntKind::BigInt {
        panic!("pycc_rt: internal error: attempted bigint dereference of a non-pointer int word")
    }
    unsafe { &*(tagged as *const BigIntObj) }
}

pub(crate) fn to_sign_and_magnitude(tagged: i64) -> (bool, Vec<u32>) {
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

/// The sign of a sign-magnitude pair as `-1`/`0`/`1`.
///
/// **Zero is decided by the magnitude, never by the `negative` flag.**
/// `bigint_add_signed` normalizes an equal-magnitude, opposite-sign result
/// to `BigIntObj { negative: false, limbs: vec![0] }`, so `!negative` does
/// *not* imply "positive" -- exactly the trap `pycc_rt_int_truthy` already
/// documents for its own magnitude inspection.
pub(crate) fn magnitude_sign(negative: bool, magnitude: &[u32]) -> i8 {
    if trim(magnitude) == [0] {
        0
    } else if negative {
        -1
    } else {
        1
    }
}

/// Orders two encoded int-compatible words (D-061/D-141) across the whole
/// representation, including heap bigints and the bool-identity markers.
///
/// Two inline operands take the cheap `i64` path so an ordinary smallint
/// `range` loop allocates nothing per iteration. As soon as either side is a
/// bigint the comparison decodes both to sign-magnitude and orders them
/// sign-first, then by magnitude (reversed for two negatives).
///
/// This is deliberately *not* `pycc_rt_int_cmp`'s general comparison
/// operator: `int_cmp` keeps its D-141 bigint boundary (see #618). This
/// helper exists for the loop-control comparison `range_continue` performs,
/// which #147 makes bigint-capable.
pub(crate) fn encoded_int_cmp(a: i64, b: i64) -> std::cmp::Ordering {
    if let (Some(a), Some(b)) = (inline_int_value(a), inline_int_value(b)) {
        return a.cmp(&b);
    }
    let (a_negative, a_magnitude) = to_sign_and_magnitude(a);
    let (b_negative, b_magnitude) = to_sign_and_magnitude(b);
    let a_sign = magnitude_sign(a_negative, &a_magnitude);
    let b_sign = magnitude_sign(b_negative, &b_magnitude);
    if a_sign != b_sign {
        return a_sign.cmp(&b_sign);
    }
    match a_sign {
        0 => std::cmp::Ordering::Equal,
        1 => magnitude_cmp(&a_magnitude, &b_magnitude),
        // Both negative: the larger magnitude is the smaller value.
        _ => magnitude_cmp(&b_magnitude, &a_magnitude),
    }
}
