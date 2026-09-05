//! Unit tests for #934's protocol-return gate as reached through a *protocol
//! member declaration* (`protocol::lower_protocol_class`'s two
//! `lower_return_annotation` call sites, with and without `self`).
//!
//! A member declared `-> P` (another protocol) is rejected deliberately, not
//! by accident of the shared function: `pycc_types::class::
//! check_protocol_conformance` compares member return types with plain
//! `is_assignable`, so a concrete `def clone(self) -> C` would not satisfy a
//! member `-> P` today, and the only satisfying spelling (`-> P` on the
//! concrete class) is itself gated. Accepting the member would declare an
//! unsatisfiable protocol. The message carries no "annotate the concrete
//! class" advice for the same reason -- that advice is wrong for an
//! interface declaration.
//!
//! Sibling of `class::tests`, in its own file per AGENTS.md's
//! decomposability rule (`class.rs` is well past the threshold).

use super::tests::lower_ok;
use crate::lower_checked;

const TWO_PROTOCOLS: &str =
    "from typing import Protocol\nclass P(Protocol):\n    def foo(self) -> int: ...\n";

const MESSAGE_P: &str = "a protocol class (`P`) as a return type annotation is not supported yet -- a protocol type is currently supported in parameter and variable positions only";

fn assert_protocol_return_c0001(source: &str) {
    let module = crate::pycc_parser_test_helper::parse(source);
    let diagnostic = lower_checked(&module).unwrap_err();
    assert_eq!(diagnostic.code, "C0001", "source: {source:?}");
    // The message, not just the code: the `Frobnicate` tests in
    // `class::tests` reach the same two `?` sites with a *different*
    // `C0001`, so the code alone would not prove the new gate fired.
    assert_eq!(diagnostic.message, MESSAGE_P, "source: {source:?}");
    let start = source
        .rfind("-> P:")
        .expect("source carries the annotation")
        + 3;
    let start = u32::try_from(start).expect("test source fits a span");
    assert_eq!(
        diagnostic.span,
        Some(pycc_diag::Span::new(start, start + 1)),
        "source: {source:?}"
    );
}

#[test]
fn a_protocol_member_with_self_returning_another_protocol_is_c0001() {
    assert_protocol_return_c0001(&format!(
        "{TWO_PROTOCOLS}class Q(Protocol):\n    def clone(self) -> P: ...\n"
    ));
}

#[test]
fn a_protocol_member_without_self_returning_another_protocol_is_c0001() {
    // The no-`self` branch of `lower_protocol_class` lowers every
    // parameter and the return annotation through the same two helpers.
    assert_protocol_return_c0001(&format!(
        "{TWO_PROTOCOLS}class Q(Protocol):\n    def clone() -> P: ...\n"
    ));
}

#[test]
fn a_self_referential_protocol_member_return_still_lowers() {
    // Pins that the gate does *not* fire here, and why: inside `class P`,
    // `annotation_to_ty`'s enclosing-class-name arm resolves `P` to
    // `Ty::Instance("P")` before the `class_defs` protocol lookup would
    // have produced `Ty::Protocol("P")`. The member therefore lowers as
    // returning an instance of `P`. (That every conforming class is then
    // rejected by `pycc_types` with a spurious `T0046` is a pre-existing
    // defect independent of #934, tracked separately.)
    let hir = lower_ok(
        "from typing import Protocol\nclass P(Protocol):\n    def clone(self) -> P: ...\n",
    );
    let (_, class_def) = hir
        .class_defs
        .iter()
        .find(|(name, _)| name == "P")
        .expect("the protocol is registered");
    let clone_return_ty = class_def
        .protocol_members
        .iter()
        .find_map(|member| match member {
            crate::ProtocolMember::Method {
                name, return_ty, ..
            } if name == "clone" => Some(return_ty.clone()),
            _ => None,
        });
    assert_eq!(
        clone_return_ty,
        Some(crate::Ty::Instance(Box::new("P".to_string())))
    );
}
