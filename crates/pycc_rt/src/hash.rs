//! `hash()` of an `int`, a `bool` and a tuple of them (#1331, Part 1 of
//! #1327): CPython 3.14's own `long_hash` and `tuplehash`, bit for bit on a
//! 64-bit build.
//!
//! Its own module rather than more of `lib.rs`, under AGENTS.md's "keep
//! source files decomposable" rule. Two entry points:
//!
//! - [`pycc_rt_hash_int`] hashes one encoded int word (D-061/D-141): the
//!   value's magnitude reduced modulo `2**61 - 1`, the sign reapplied, and
//!   `-1` mapped to `-2`, exactly as `Objects/longobject.c`'s `long_hash`.
//! - [`pycc_rt_hash_tuple`] folds already-computed element hashes ("lanes")
//!   with the xxHash-derived accumulator of `Objects/tupleobject.c`'s
//!   `tuplehash`. `pycc_codegen` computes every lane first, because a tuple
//!   is an SSA struct of known arity (D-115/D-116), and makes one call.
//!
//! Both return a raw `i64`; `pycc_codegen` encodes it with
//! `pycc_rt_int_from_i64`, which promotes a tuple hash outside the smallint
//! range to a heap bigint. Neither function allocates a result, raises or
//! releases its argument.

use crate::int_encoding::{inline_int_value, to_sign_and_magnitude};

/// `_PyHASH_MODULUS`, the Mersenne prime `2**61 - 1` CPython reduces every
/// numeric hash by on a 64-bit build.
const MODULUS: u64 = (1 << 61) - 1;

/// `tuplehash`'s `_PyHASH_XXPRIME_1`.
const XXPRIME_1: u64 = 11_400_714_785_074_694_791;
/// `tuplehash`'s `_PyHASH_XXPRIME_2`.
const XXPRIME_2: u64 = 14_029_467_366_897_019_727;
/// `tuplehash`'s `_PyHASH_XXPRIME_5`, also the accumulator's seed.
const XXPRIME_5: u64 = 2_870_177_450_012_600_261;

/// The magnitude `limbs` (base `2**32`, little-endian) reduced modulo
/// [`MODULUS`]. CPython iterates 30-bit digits with a rotate-and-add;
/// reducing the same number with 32-bit limbs through `x * 2**32 + limb`
/// gives the same residue because the reduction is of the numeric value.
fn reduce_magnitude(limbs: &[u32]) -> u64 {
    limbs.iter().rev().fold(0, |x, &limb| {
        (((u128::from(x) << 32) | u128::from(limb)) % u128::from(MODULUS)) as u64
    })
}

/// CPython's `hash(n)` for the encoded int word `word` (D-061/D-141). The
/// two bool markers decode to `0` and `1` like any smallint. Borrows
/// `word`: it neither retains nor releases a heap bigint.
#[unsafe(no_mangle)]
pub extern "C" fn pycc_rt_hash_int(word: i64) -> i64 {
    let (negative, residue) = match inline_int_value(word) {
        Some(value) => (value < 0, value.unsigned_abs() % MODULUS),
        None => {
            let (negative, limbs) = to_sign_and_magnitude(word);
            (negative, reduce_magnitude(&limbs))
        }
    };
    // `residue < 2**61`, so the cast and the negation cannot overflow.
    let hash = if negative {
        -(residue as i64)
    } else {
        residue as i64
    };
    if hash == -1 { -2 } else { hash }
}

/// CPython's `hash(t)` for a tuple whose element hashes are `lanes[..len]`.
///
/// # Safety
/// `lanes` must be non-null, aligned for `i64` and valid for `len` reads --
/// including when `len` is `0`, which `std::slice::from_raw_parts` requires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_hash_tuple(lanes: *const i64, len: usize) -> i64 {
    // SAFETY: the caller guarantees `lanes` is valid for `len` reads.
    let lanes = unsafe { std::slice::from_raw_parts(lanes, len) };
    let acc = lanes.iter().fold(XXPRIME_5, |acc, &lane| {
        acc.wrapping_add((lane as u64).wrapping_mul(XXPRIME_2))
            .rotate_left(31)
            .wrapping_mul(XXPRIME_1)
    });
    let acc = acc.wrapping_add(len as u64 ^ (XXPRIME_5 ^ 3_527_539));
    if acc == u64::MAX {
        1_546_275_796
    } else {
        acc as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::int_encoding::{BigIntObj, bigint_release, tag_bigint, tag_smallint};

    /// A heap bigint word with sign `negative` and base-`2**32` magnitude
    /// `limbs`, built directly so magnitudes pycc's arithmetic cannot yet
    /// reach (`2**70`, `2**100`) are testable.
    fn big(negative: bool, limbs: &[u32]) -> i64 {
        tag_bigint(BigIntObj::new(negative, limbs.to_vec()))
    }

    fn hash_big(negative: bool, limbs: &[u32]) -> i64 {
        let word = big(negative, limbs);
        let hash = pycc_rt_hash_int(word);
        bigint_release(word);
        hash
    }

    /// CPython's own `long_hash` recurrence over 30-bit digits, the
    /// formulation R1 of the plan cross-checks the 32-bit reduction with.
    fn cpython_digit_residue(limbs: &[u32]) -> u64 {
        let mut value: u128 = 0;
        for &limb in limbs.iter().rev() {
            value = (value << 32) | u128::from(limb);
            // Only small test magnitudes (< 2**96) are fed here.
        }
        let mut digits = Vec::new();
        while value != 0 {
            digits.push((value & ((1 << 30) - 1)) as u64);
            value >>= 30;
        }
        let mut x: u64 = 0;
        for digit in digits.iter().rev() {
            x = ((x << 30) & MODULUS) | (x >> (61 - 30));
            // `x < 2**61 + 2**30` here, so this is CPython's single
            // conditional subtraction of the modulus.
            x = (x + digit) % MODULUS;
        }
        x
    }

    #[test]
    fn smallints_and_bool_markers_match_cpython() {
        assert_eq!(pycc_rt_hash_int(tag_smallint(0)), 0);
        assert_eq!(pycc_rt_hash_int(tag_smallint(5)), 5);
        assert_eq!(pycc_rt_hash_int(tag_smallint(-1)), -2);
        assert_eq!(pycc_rt_hash_int(tag_smallint(-2)), -2);
        assert_eq!(pycc_rt_hash_int(tag_smallint((1 << 61) - 1)), 0);
        assert_eq!(pycc_rt_hash_int(tag_smallint(1 << 61)), 1);
        assert_eq!(pycc_rt_hash_int(tag_smallint(-(1 << 61))), -2);
        assert_eq!(pycc_rt_hash_int(tag_smallint((1 << 62) - 1)), 1);
        assert_eq!(pycc_rt_hash_int(tag_smallint(-(1 << 62))), -2);
        assert_eq!(pycc_rt_hash_int(2), 0, "False");
        assert_eq!(pycc_rt_hash_int(6), 1, "True");
    }

    #[test]
    fn heap_bigints_of_both_signs_match_cpython() {
        // 2**62, 2**63, 2**64, 2**70, 2**100.
        assert_eq!(hash_big(false, &[0, 1 << 30]), 2);
        assert_eq!(hash_big(false, &[0, 1 << 31]), 4);
        assert_eq!(hash_big(false, &[0, 0, 1]), 8);
        assert_eq!(hash_big(false, &[0, 0, 1 << 6]), 512);
        assert_eq!(hash_big(false, &[0, 0, 0, 1 << 4]), 549_755_813_888);
        assert_eq!(hash_big(true, &[0, 0, 0, 1 << 4]), -549_755_813_888);
        // -(2**62) as a heap word maps through -2 like the smallint.
        assert_eq!(hash_big(true, &[0, 1 << 30]), -2);
        // A bigint whose magnitude is exactly P hashes to 0, of either sign.
        assert_eq!(hash_big(true, &[u32::MAX, (1 << 29) - 1]), 0);
    }

    #[test]
    fn the_32_bit_reduction_agrees_with_cpythons_30_bit_digits() {
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..2000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let limbs = [state as u32, (state >> 32) as u32, (state >> 17) as u32];
            assert_eq!(
                reduce_magnitude(&limbs),
                cpython_digit_residue(&limbs),
                "{limbs:?}"
            );
        }
    }

    fn tuple(lanes: &[i64]) -> i64 {
        unsafe { pycc_rt_hash_tuple(lanes.as_ptr(), lanes.len()) }
    }

    #[test]
    fn tuple_hashes_match_cpython() {
        assert_eq!(tuple(&[]), 5_740_354_900_026_072_187);
        assert_eq!(tuple(&[-2]), 8_078_679_518_589_016_365);
        assert_eq!(tuple(&[1, 2]), -3_550_055_125_485_641_917);
        assert_eq!(tuple(&[1, 0]), -5_164_621_852_614_943_976);
        assert_eq!(tuple(&[1, -2]), -6_779_188_579_744_246_035);
        let inner = tuple(&[1, 2]);
        assert_eq!(tuple(&[inner, 3]), -333_907_151_259_015_829);
    }
}
