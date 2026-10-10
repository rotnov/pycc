//! #1517 (Part 3 of #1514): a positional `obj.method(args)` call reaches
//! CPython without building a bound method -- one
//! `pycc_ext_obj_method_lookup` with per-call-site state, then one
//! `pycc_ext_obj_method_call` over an argument array whose leading slot is
//! reserved for the receiver.
//!
//! `use super::*` reaches the parent's private `call`, `entry_ir` and
//! `compiled_ir` helpers.

use super::*;

/// The number of non-overlapping occurrences of `needle` in `haystack`.
fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// The hot path's shape, which is the regression #1517 removes: a
/// positional method call no longer reaches `pycc_ext_obj_getattr` (whose
/// result, for a method, is a freshly allocated bound method) nor
/// `pycc_ext_obj_call` (which released that bound method again). Both
/// symbols are matched with their call parenthesis, since the new
/// symbols are deliberately not spelled to contain either one.
#[test]
fn a_positional_method_call_builds_no_bound_method() {
    let ir = entry_ir(
        "foreign_method_no_bound",
        call("gc", "set_threshold", vec![MirExpr::IntLiteral(1)]),
    );
    assert_eq!(
        occurrences(&ir, &format!("@{EXT_OBJ_METHOD_LOOKUP_SYMBOL}(")),
        1,
        "{ir}"
    );
    assert_eq!(
        occurrences(&ir, &format!("@{EXT_OBJ_METHOD_CALL_SYMBOL}(")),
        1,
        "{ir}"
    );
    assert!(!ir.contains(&format!("@{EXT_OBJ_GETATTR_SYMBOL}(")), "{ir}");
    assert!(!ir.contains(&format!("@{EXT_OBJ_CALL_SYMBOL}(")), "{ir}");
}

/// Each call site owns one internal, zero-initialised `[2 x ptr]` of
/// state -- the receiver type it last saw and that type's unbound method
/// -- which is the state the shim's first-use fill expects. Two sites
/// calling one name get two states, so a site that sees a `list` and one
/// that sees a `dict` do not evict each other's entry; both still share
/// the one interned-name slot (#1515).
#[test]
fn each_call_site_owns_its_own_zeroed_site_state() {
    let mut items = call("gc", "collect", Vec::new());
    items.extend(call("gc", "collect", Vec::new()));
    let (entry, whole) = compiled_ir("foreign_method_sites", items);
    // LLVM suffixes a repeated global name with a module-wide counter, so
    // the second site's exact suffix is not pinned -- only that there are
    // two distinct ones, each defined zeroed and passed by exactly one call.
    let sites: Vec<&str> = whole
        .lines()
        .filter(|line| line.starts_with("@pycc_foreign_method_site.collect"))
        .map(|line| {
            let (name, definition) = line.split_once(" = ").expect("a global definition");
            assert_eq!(
                definition, "internal global [2 x ptr] zeroinitializer",
                "{whole}"
            );
            name
        })
        .collect();
    assert_eq!(sites.len(), 2, "{whole}");
    assert_ne!(sites[0], sites[1], "{whole}");
    for site in sites {
        assert_eq!(occurrences(&entry, &format!("ptr {site},")), 1, "{entry}");
    }
    assert_eq!(
        occurrences(&entry, "ptr @pycc_foreign_attr_slot.collect,"),
        2,
        "{entry}"
    );
}

/// The argument array has one slot more than the call has arguments: slot
/// 0 is reserved for the receiver an unbound method descriptor needs (or
/// for the callee's own use under `PY_VECTORCALL_ARGUMENTS_OFFSET`), so the
/// packed arguments land at 1..=n and the arity passed is n, not n + 1.
#[test]
fn the_argument_array_reserves_a_leading_receiver_slot() {
    let ir = entry_ir(
        "foreign_method_leading_slot",
        call(
            "gc",
            "set_threshold",
            vec![MirExpr::IntLiteral(1), MirExpr::IntLiteral(2)],
        ),
    );
    assert!(ir.contains("alloca ptr, i64 3"), "{ir}");
    let slots: Vec<&str> = ir
        .lines()
        .filter(|line| line.contains("getelementptr inbounds ptr, ptr"))
        .filter_map(|line| line.rsplit(", i64 ").next())
        .collect();
    assert_eq!(
        slots,
        ["1", "2"],
        "slot 0 is never written by the compiled code: {ir}"
    );
    let call_at = ir
        .find(&format!("@{EXT_OBJ_METHOD_CALL_SYMBOL}("))
        .unwrap_or_else(|| panic!("no method call was emitted: {ir}"));
    let call_line = ir[call_at..].lines().next().unwrap_or_default();
    assert!(call_line.ends_with(", i64 2)"), "{call_line}");
}

/// The receiver the lookup hands back travels through a one-pointer
/// out-parameter allocated in the entry block (an `alloca` anywhere else
/// would grow the stack on every loop iteration), is loaded straight after
/// the lookup, and reaches the call as its second operand.
#[test]
fn the_receiver_reaches_the_call_through_an_entry_block_out_parameter() {
    let ir = entry_ir(
        "foreign_method_receiver",
        call("gc", "set_threshold", vec![MirExpr::IntLiteral(1)]),
    );
    let first_branch = ir.find("br ").expect("the entry block branches");
    let entry_block = &ir[..first_branch];
    assert!(entry_block.contains("alloca ptr, i64 1"), "{ir}");
    assert!(entry_block.contains("alloca ptr, i64 2"), "{ir}");
    let lookup_at = ir
        .find(&format!("@{EXT_OBJ_METHOD_LOOKUP_SYMBOL}("))
        .expect("a lookup is emitted");
    let load_at = ir
        .find("%foreign_call_receiver = load ptr")
        .expect("the receiver is loaded");
    assert!(lookup_at < load_at, "{ir}");
    assert!(
        ir.contains(&format!(
            "@{EXT_OBJ_METHOD_CALL_SYMBOL}(ptr %foreign_call_callable, ptr %foreign_call_receiver,"
        )),
        "{ir}"
    );
}
