//! Unit tests for the crate-root HIR-to-MIR lowering surface.
//!
//! Split into cohesion-driven submodules (issue #546), each grouped around the
//! production seam it exercises. Every submodule is still a descendant of the
//! crate root, so its `use crate::*` reaches exactly the private items the
//! single flat `mod tests` reached through `use super::*`.

mod builtin;
mod class_attr;
mod class_dunder;
mod class_getitem;
mod class_mro;
mod class_predicate;
mod class_static;
mod class_super;
mod collection;
mod comprehension;
mod exception;
mod expr;
mod expr_ty;
mod matching;
mod narrow;
mod protocol;
mod scope;
mod slice;
mod stmt;
