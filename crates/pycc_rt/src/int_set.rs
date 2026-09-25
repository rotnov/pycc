//! `set[int]`'s and `frozenset[int]`'s shared runtime representation,
//! `PyIntSetObj`, and every `pycc_rt_int_set_*` entry point (D-121/D-141,
//! D-123). Extracted from `lib.rs` under AGENTS.md's file-decomposition rule
//! when Part 1 of #1319 added the two copy constructors `frozenset(...)`
//! lowers to; the moved items keep their code and `#[unsafe(no_mangle)]`
//! symbols verbatim. `frozenset[int]` shares this representation because it
//! differs from `set[int]` only in mutability, which `pycc_types` enforces.

use super::*;

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
    // Decode *before* taking `items` out of the `Cell`: an early return
    // between the `take()` and the matching `set()` would leave the set
    // silently emptied.
    let Some(value_numeric) = decode_inline_or_raise(value, "storing in set[int]") else {
        return;
    };
    let mut items = unsafe { &*set }.items.take();
    if !items
        .iter()
        .copied()
        // Already-stored words passed the ingress check above, so none of
        // them is a bigint; `inline_int_value` needs no raise of its own and
        // a hypothetical bigint simply compares unequal.
        .any(|existing| inline_int_value(existing) == Some(value_numeric))
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

/// Raises `RuntimeError` (D-173) if `current_len` differs from
/// `expected_len`. `ForSet`'s own
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
/// iteration` here, and Part B of #1038 (#1064) makes this do the same:
/// a D-173 pending-exception raise with CPython's own message, not an
/// abort. The function returns `()`; the `ForSet` loop-test codegen
/// terminates the loop by conjoining `pycc_rt_exception_active() == 0`
/// onto its continue condition.
///
/// A pending exception suppresses the check entirely. `pycc_rt_exception_raise`
/// replaces the thread-local pending value unconditionally, so a body that both
/// grows the set *and* raises -- `for x in s: s.add(x + 1); xs.pop()` on an
/// empty `xs` -- would otherwise reach this check with `IndexError` pending and
/// leave with `RuntimeError` pending, selecting the wrong `except` handler
/// (CPython propagates the body's own `IndexError`). Suppressing here rather
/// than reordering the loop-test codegen costs nothing in correctness: the
/// loop-test's `pycc_rt_exception_active() == 0` conjunct is evaluated on the
/// same iteration and terminates the loop either way, so the only observable
/// difference is which exception survives.
pub(crate) fn check_set_len_unchanged(current_len: i64, expected_len: i64) {
    if pycc_rt_exception_active() != 0 {
        return;
    }
    if current_len != expected_len {
        raise_builtin(
            EXCEPTION_TYPE_RUNTIME_ERROR,
            "RuntimeError",
            "Set changed size during iteration",
        );
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

/// Builds a new `PyIntSetObj` holding `src`'s elements in `src`'s order,
/// with refcount `1` (Part 1 of #1319): `frozenset(s)` for a `set[int]` or
/// `frozenset[int]` source. `src` is left untouched. A fresh object rather
/// than `src` itself, even though CPython returns an exact frozenset
/// argument unchanged: identity on a container is not observable in the
/// compiled subset (`is` is not admitted), and a copy is what a `set`
/// source needs anyway.
///
/// # Safety
/// `src` must be a live `PyIntSetObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_set_copy(src: *mut PyIntSetObj) -> *mut PyIntSetObj {
    let items = unsafe { &*src }.items.take();
    let copy = items.clone();
    unsafe { &*src }.items.set(items);
    Box::into_raw(Box::new(PyIntSetObj {
        rc: Cell::new(1),
        items: Cell::new(copy),
    }))
}

/// Builds a new `PyIntSetObj` from `src`'s elements with refcount `1`
/// (Part 1 of #1319): `frozenset(xs)` for a `list[int]` source. Every
/// element goes through [`pycc_rt_int_set_add`], so duplicates keep their
/// first occurrence exactly as a set literal does, and a bigint word raises
/// exactly as `pycc_rt_int_set_add` raises. A list reaching this call never
/// holds a bigint in practice -- codegen's ingress validation refuses one at
/// insertion -- so that raise path is defensive; it stops at the first
/// raise and returns the partially filled set for the caller's pending-
/// exception check.
///
/// # Safety
/// `src` must be a live `PyIntListObj` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_int_set_from_int_list(src: *mut PyIntListObj) -> *mut PyIntSetObj {
    let items = unsafe { &*src }.items.take();
    let values = items.clone();
    unsafe { &*src }.items.set(items);
    let set = pycc_rt_int_set_new();
    for value in values {
        unsafe { pycc_rt_int_set_add(set, value) };
        if pycc_rt_exception_active() != 0 {
            break;
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{a_bigint_word, assert_overflow_raised};

    fn int_list_of(values: &[i64]) -> *mut PyIntListObj {
        let list = pycc_rt_int_list_new();
        let words: Vec<i64> = values.iter().map(|value| tag_smallint(*value)).collect();
        unsafe { &*list }.items.set(words);
        list
    }

    fn set_items(set: *mut PyIntSetObj) -> Vec<i64> {
        let len = unsafe { pycc_rt_int_set_len(set) };
        (0..len)
            .map(|index| unsafe { pycc_rt_int_set_get(set, index) })
            .collect()
    }

    #[test]
    fn copy_of_an_empty_set_is_a_distinct_empty_set() {
        unsafe {
            let src = pycc_rt_int_set_new();
            let copy = pycc_rt_int_set_copy(src);
            assert_ne!(src, copy);
            assert_eq!(pycc_rt_int_set_len(copy), 0);
            pycc_rt_int_set_decref(src);
            pycc_rt_int_set_decref(copy);
        }
    }

    #[test]
    fn copy_keeps_order_and_leaves_the_source_independent() {
        unsafe {
            let src = pycc_rt_int_set_new();
            pycc_rt_int_set_add(src, tag_smallint(3));
            pycc_rt_int_set_add(src, tag_smallint(1));
            let copy = pycc_rt_int_set_copy(src);
            pycc_rt_int_set_add(src, tag_smallint(9));
            assert_eq!(set_items(copy), vec![tag_smallint(3), tag_smallint(1)]);
            assert_eq!(pycc_rt_int_set_len(src), 3);
            pycc_rt_int_set_decref(src);
            pycc_rt_int_set_decref(copy);
        }
    }

    #[test]
    fn from_int_list_dedups_keeping_first_occurrence() {
        unsafe {
            let empty = int_list_of(&[]);
            let from_empty = pycc_rt_int_set_from_int_list(empty);
            assert_eq!(pycc_rt_int_set_len(from_empty), 0);

            let list = int_list_of(&[2, 1, 2, 1, 5]);
            let set = pycc_rt_int_set_from_int_list(list);
            assert_eq!(
                set_items(set),
                vec![tag_smallint(2), tag_smallint(1), tag_smallint(5)]
            );
            // The source list is left intact.
            assert_eq!(pycc_rt_int_list_len(list), 5);
            pycc_rt_int_set_decref(from_empty);
            pycc_rt_int_set_decref(set);
        }
    }

    #[test]
    fn from_int_list_raises_on_a_bigint_word_and_stops() {
        pycc_rt_exception_clear();
        unsafe {
            let list = int_list_of(&[4]);
            let big = tag_bigint(bigint_from_i128(1i128 << 100));
            let mut words = { &*list }.items.take();
            words.push(big);
            words.push(tag_smallint(7));
            { &*list }.items.set(words);
            let set = pycc_rt_int_set_from_int_list(list);
            assert_ne!(pycc_rt_exception_active(), 0);
            assert_eq!(set_items(set), vec![tag_smallint(4)]);
            pycc_rt_int_set_decref(set);
        }
        pycc_rt_exception_clear();
    }

    #[test]
    fn a_bigint_value_added_to_an_int_set_raises_and_leaves_the_set_intact() {
        // The `Cell::take` hazard: `pycc_rt_int_set_add` lifts `items` out
        // of its cell, so an early return placed after the `take()` would
        // leave the set permanently empty. Decoding first is what keeps the
        // already-stored element below observable.
        pycc_rt_exception_clear();
        let set = pycc_rt_int_set_new();
        unsafe { pycc_rt_int_set_add(set, tag_smallint(7)) };
        unsafe { pycc_rt_int_set_add(set, a_bigint_word()) };
        assert_overflow_raised("storing in set[int]");
        assert_eq!(unsafe { pycc_rt_int_set_len(set) }, 1);
        pycc_rt_exception_clear();
        unsafe { pycc_rt_int_set_decref(set) };
    }

    #[test]
    fn pycc_rt_int_set_check_not_resized_is_a_no_op_when_lengths_match() {
        // Calls the public wrapper directly (safe for the non-panicking
        // path, unlike the panic-path test below), so the wrapper's own
        // call-through line is exercised too, not just the private helper.
        pycc_rt_int_set_check_not_resized(3, 3);
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
        // test in `lib.rs`.
        unsafe {
            pycc_rt_int_set_incref(std::ptr::null_mut());
            pycc_rt_int_set_decref(std::ptr::null_mut());
        }
    }
}
