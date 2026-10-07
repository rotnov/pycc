//! Return coverage: whether a function body can reach its implicit end.
//!
//! [`block_always_returns`] decides the `T0022` "function `f` can exit
//! without returning `T`" check in `check_function_in`, and the
//! fall-through joins after a `try` statement
//! (`exception::try_join`, `constraints::try_stmt`) reuse it to drop a
//! path that never reaches the statement after it. The predicate itself is
//! `pycc_hir`'s, shared with `pycc_mir`'s `Try` lowering (#1476).

pub(crate) use pycc_hir::block_always_returns;
