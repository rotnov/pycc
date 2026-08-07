//! Class instance runtime object (D-154, Part 1 of #375; see the
//! class-instance-layout ADR): `PyInstanceObj` and its `extern "C"`
//! accessor functions.
//!
//! Follows the exact opaque-heap-object-with-FFI-accessors shape every
//! other `pycc_rt` container object (`PyStrObj`, `PyIntListObj`,
//! `PyDictObj`, `PyIntSetObj`) already uses: `PyInstanceObj` is `pub` only
//! so it can appear in an `extern "C"` function signature (the
//! `private_interfaces` lint, a hard error under this workspace's `-D
//! warnings` clippy gate, refuses a private type there) -- its field stays
//! private, so nothing outside this module can construct one or read its
//! contents except through the functions below. It is deliberately **not**
//! `#[repr(C)]`: `pycc_codegen` never reads or writes a slot by computing a
//! field offset itself (no LLVM `GEP` against this struct anywhere), only
//! by calling [`pycc_rt_instance_get_slot`]/[`pycc_rt_instance_set_slot`]
//! with a slot index `pycc_mir` already resolved at compile time against the
//! class's declared attribute list -- exactly the ADR's own recommended
//! fork (opaque-with-accessors, not a cross-crate ABI layout commitment).
//!
//! **Slot representation.** Every slot is a plain `i64` word, matching this
//! crate's own existing `PyIntListObj` element representation. `int`/`bool`
//! attributes store their value directly; a `float` attribute stores its
//! `f64::to_bits()` bit pattern (`pycc_codegen` bitcasts around the call,
//! there is no separate float-typed accessor pair to keep this module's own
//! FFI surface minimal); a heap-object-typed attribute (`str`, or another
//! class instance) stores its pointer, reinterpreted as an `i64` (valid on
//! every target this workspace compiles for, where a pointer and `i64` are
//! both 8 bytes) -- `pycc_codegen` handles the `inttoptr`/`ptrtoint`
//! conversion around the call, mirroring the float bitcast.
//!
//! **No reference counting.** Per the class-instance-layout ADR and this
//! issue's own explicit scope ("no new garbage-collection/reference-
//! counting design beyond what `pycc_rt`'s existing heap objects already
//! do"): `PyIntListObj`/`PyDictObj`/`PyIntSetObj` all define an `incref`/
//! `decref` pair, but `pycc_codegen` never actually calls any of them (D-107,
//! confirmed directly in `pycc_codegen`'s own doc comments) -- every
//! container value already leaks in practice, refcounting exists only as
//! unused-but-present infrastructure. `PyInstanceObj` does not even define
//! that unused pair, and deliberately carries no `rc` field the way
//! `PyIntListObj`/`PyDictObj`/`PyIntSetObj` do: an `rc` field with no
//! `incref`/`decref` pair ever reading or writing it past construction is
//! dead code (`-D warnings` rejects it outright), not merely unused-but-
//! harmless infrastructure -- so matching the *effective* (not aspirational)
//! behavior of every other heap object here means omitting the field
//! entirely, not carrying an inert placeholder. A future PR that wires up
//! real project-wide refcounting adds `rc` back alongside its own
//! `incref`/`decref` pair, exactly as it must add the same to every other
//! heap object in this file.

use std::cell::Cell;

/// See this module's own doc comment for the opacity/layout and
/// no-refcounting rationale.
pub struct PyInstanceObj {
    slots: Cell<Vec<i64>>,
}

/// Allocates a fresh instance with `slot_count` slots, each initialized to
/// `0` (Python has no "uninitialized attribute" read -- every declared slot
/// is always assigned by `__init__` before any read can observe it,
/// enforced by `pycc_types`' own first-assignment-order attribute tracking
/// -- this zero-initialization is defense in depth, not a load-bearing
/// default value). A negative `slot_count` is an internal-error panic
/// (impossible from real `pycc_codegen` output, which only ever emits a
/// class's own non-negative declared attribute count as a compile-time
/// constant), held in this private function so the panic-across-FFI split
/// matches every other bounds check in this module (see
/// `instance_get_slot`'s own doc comment).
fn new_instance(slot_count: i64) -> PyInstanceObj {
    let slot_count = usize::try_from(slot_count).unwrap_or_else(|_| {
        panic!("pycc_rt: internal error: negative instance slot count {slot_count}")
    });
    PyInstanceObj {
        slots: Cell::new(vec![0; slot_count]),
    }
}

/// # Safety
/// `slot_count` must be non-negative -- true for every `pycc_codegen` call
/// site, which always passes a compile-time-constant, non-negative class
/// attribute count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_instance_new(slot_count: i64) -> *mut PyInstanceObj {
    Box::into_raw(Box::new(new_instance(slot_count)))
}

/// # Safety (panic-across-FFI note, same rationale as `pycc_rt_int_add`'s
/// own doc comment)
/// `pycc_rt_instance_get_slot` below is a plain `extern "C" fn`, not `extern
/// "C-unwind"`. A panic that would otherwise unwind past its boundary is
/// instead turned into a process abort -- correct for pycc-generated LLVM
/// code calling it (an out-of-range `slot` is impossible from a
/// `pycc_types`-checked program, since every `AttrGet`/`AttrSet` slot index
/// is resolved at compile time against the class's own declared attribute
/// count), but unsuitable for a `#[should_panic]` test directly against the
/// public wrapper (confirmed empirically the same way
/// `pycc_rt_float_to_str`'s own split was: see this module's own tests
/// below). This private function holds the real, freely-panicking logic.
/// Takes `slot` as the raw `i64` the FFI boundary receives, exactly like
/// `int_list_get`'s own `index: i64` -- a negative `slot` and an
/// out-of-range `slot` are both "index out of range" here, matching that
/// function's own combined bounds check, not two separate failure modes.
fn instance_get_slot(instance: &PyInstanceObj, slot: i64) -> i64 {
    let slots = instance.slots.take();
    if slot < 0 || slot as usize >= slots.len() {
        let len = slots.len();
        instance.slots.set(slots);
        panic!(
            "pycc_rt: internal error: instance slot {slot} out of range (instance has {len} \
             slots) -- pycc_mir should have resolved a slot index within the class's own \
             declared attribute count"
        );
    }
    let value = slots[slot as usize];
    instance.slots.set(slots);
    value
}

/// Reads slot `slot`'s raw `i64` word -- see this module's own doc comment
/// for what that word means for each attribute type.
///
/// # Safety
/// `instance` must be a live `PyInstanceObj` pointer -- every `pycc_codegen`
/// call site only ever passes a value it just evaluated from a
/// well-typed `Ty::Instance` expression.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_instance_get_slot(instance: *mut PyInstanceObj, slot: i64) -> i64 {
    instance_get_slot(unsafe { &*instance }, slot)
}

/// Writes `value` into slot `slot`, overwriting whatever was there --
/// mirrors [`instance_get_slot`]'s own panic-across-FFI split and
/// out-of-range reasoning.
fn instance_set_slot(instance: &PyInstanceObj, slot: i64, value: i64) {
    let mut slots = instance.slots.take();
    if slot < 0 || slot as usize >= slots.len() {
        let len = slots.len();
        instance.slots.set(slots);
        panic!(
            "pycc_rt: internal error: instance slot {slot} out of range (instance has {len} \
             slots) -- pycc_mir should have resolved a slot index within the class's own \
             declared attribute count"
        );
    }
    slots[slot as usize] = value;
    instance.slots.set(slots);
}

/// # Safety
/// `instance` must be a live `PyInstanceObj` pointer, same requirement as
/// [`pycc_rt_instance_get_slot`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pycc_rt_instance_set_slot(
    instance: *mut PyInstanceObj,
    slot: i64,
    value: i64,
) {
    instance_set_slot(unsafe { &*instance }, slot, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_instance_has_every_slot_zero_initialized() {
        let instance = unsafe { &*pycc_rt_instance_new(3) };
        assert_eq!(instance_get_slot(instance, 0), 0);
        assert_eq!(instance_get_slot(instance, 1), 0);
        assert_eq!(instance_get_slot(instance, 2), 0);
    }

    #[test]
    fn setting_a_slot_is_observable_by_a_later_get() {
        let instance = unsafe { &*pycc_rt_instance_new(2) };
        instance_set_slot(instance, 0, 42);
        instance_set_slot(instance, 1, -7);
        assert_eq!(instance_get_slot(instance, 0), 42);
        assert_eq!(instance_get_slot(instance, 1), -7);
    }

    #[test]
    fn setting_one_slot_does_not_disturb_another() {
        let instance = unsafe { &*pycc_rt_instance_new(2) };
        instance_set_slot(instance, 0, 1);
        instance_set_slot(instance, 1, 2);
        instance_set_slot(instance, 0, 99);
        assert_eq!(instance_get_slot(instance, 0), 99);
        assert_eq!(instance_get_slot(instance, 1), 2);
    }

    #[test]
    fn a_zero_slot_instance_allocates_without_panicking() {
        // A class could in principle declare no attributes at all (not
        // reachable from this PR's own HIR lowering, which requires
        // `__init__`, but not that it assign any attribute) -- confirm the
        // degenerate zero-slot case is not an internal-error trap.
        let instance = unsafe { &*pycc_rt_instance_new(0) };
        let slots = instance.slots.take();
        assert!(slots.is_empty());
        instance.slots.set(slots);
    }

    #[test]
    #[should_panic(expected = "instance slot 5 out of range")]
    fn reading_an_out_of_range_slot_panics_with_an_internal_error() {
        let instance = unsafe { &*pycc_rt_instance_new(2) };
        instance_get_slot(instance, 5);
    }

    #[test]
    #[should_panic(expected = "instance slot 5 out of range")]
    fn writing_an_out_of_range_slot_panics_with_an_internal_error() {
        let instance = unsafe { &*pycc_rt_instance_new(2) };
        instance_set_slot(instance, 5, 1);
    }

    #[test]
    fn the_public_get_wrapper_reads_a_slot_through_the_ffi_boundary() {
        let ptr = unsafe { pycc_rt_instance_new(1) };
        unsafe { pycc_rt_instance_set_slot(ptr, 0, 123) };
        assert_eq!(unsafe { pycc_rt_instance_get_slot(ptr, 0) }, 123);
    }

    #[test]
    #[should_panic(expected = "negative instance slot count")]
    fn a_negative_slot_count_panics_with_an_internal_error() {
        // Calls the private `new_instance` directly, not the public
        // `pycc_rt_instance_new` FFI wrapper -- that wrapper is a plain
        // `extern "C" fn`, not `extern "C-unwind"`, so a panic propagating
        // through it aborts the process instead of unwinding, the same
        // panic-across-FFI reasoning `instance_get_slot`'s own doc comment
        // documents (confirmed empirically: an earlier version of this test
        // called the public wrapper and aborted the whole test process
        // instead of being caught by `#[should_panic]`).
        new_instance(-1);
    }

    #[test]
    #[should_panic(expected = "instance slot -1 out of range")]
    fn a_negative_get_slot_index_panics_with_an_internal_error() {
        let instance = unsafe { &*pycc_rt_instance_new(1) };
        instance_get_slot(instance, -1);
    }

    #[test]
    #[should_panic(expected = "instance slot -1 out of range")]
    fn a_negative_set_slot_index_panics_with_an_internal_error() {
        let instance = unsafe { &*pycc_rt_instance_new(1) };
        instance_set_slot(instance, -1, 0);
    }
}
