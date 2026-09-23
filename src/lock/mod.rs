//! `pycc lock`: records the CPython dependency closure an embedded build
//! will carry, read offline from the `PYCC_PYTHON` interpreter's installed
//! environment, into a per-entry, per-triple `pycc.lock` (the pycc.lock
//! decision entry under `docs/decisions/`; `docs/CLI_SPEC.md` describes
//! the command).

pub(crate) mod dist;
#[cfg(test)]
pub(crate) mod fixture;
pub(crate) mod marker;
pub(crate) mod requirement;
pub(crate) mod resolve;
