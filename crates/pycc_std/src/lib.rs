//! `pycc_std`: a static registry of stdlib symbols this pycc version
//! hand-recognizes at compile time (D-136).
//!
//! This crate is deliberately a plain data crate: no proc-macro, no build
//! script, and no dependency on any other workspace crate (including
//! `pycc_hir`, which owns the compiler's real `Ty` representation). Callers
//! (`pycc_hir` for import binding, `pycc_types` for type-checking) convert
//! [`ScalarKind`] into their own `Ty` at the call site instead of this crate
//! depending on either of them -- keeping the dependency direction strictly
//! one-way (`pycc_hir`/`pycc_types` -> `pycc_std`, never the reverse).
//!
//! Scope (D-088, D-136): this PR ships exactly `math.sqrt` and `math.pi`.
//! `sys` is not registered at all -- the original plan considered
//! `sys.argv`/`sys.exit`, but both need machinery (a `list[str]`-shaped
//! runtime value for `argv`, `NoReturn`-style divergence handling in
//! `pycc_types`/`pycc_mir` for `exit`) that has no existing precedent in
//! this codebase and no concrete lowering slice in this PR's Task 4 scope.
//! Recorded as a D-136 addendum in `docs/DECISIONS.md` rather than silently
//! narrowing the ADR's own text. `math.floor`/`math.ceil` are likewise
//! withheld from the registry for the same reason: D-136 (via D-088's
//! established series precedent) requires that nothing type-checkable here
//! be left unlowerable in `pycc_mir`/`pycc_codegen`, and only `sqrt`/`pi`
//! have a concrete lowering path landing in this PR.

/// A stdlib module name this compiler recognizes as an `import`/
/// `from ... import ...` target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdModule {
    Math,
}

/// The scalar shape of a registered symbol's argument/return type, kept
/// local to this crate (D-136's dependency-direction ruling) rather than
/// reusing `pycc_hir::Ty` directly. Deliberately covers only the concrete
/// cases the registered symbols actually need -- not a general type
/// representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarKind {
    Float,
}

/// The kind of a registered stdlib symbol: a callable function with a fixed
/// argument/return shape, or a constant with a fixed type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdSymbolKind {
    Function {
        arg_tys: &'static [ScalarKind],
        ret_ty: ScalarKind,
    },
    Constant {
        ty: ScalarKind,
    },
}

/// A single registered stdlib symbol: which module it lives in, its source
/// name, and its type shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StdSymbol {
    pub module: StdModule,
    pub name: &'static str,
    pub kind: StdSymbolKind,
}

/// The full registry of hand-recognized stdlib symbols (D-136). A plain
/// `const` slice, linearly scanned by [`resolve_symbol`] -- fewer than two
/// dozen entries, so no `HashMap`/`OnceCell`/macro-generated dispatch table
/// is warranted, and a linear scan keeps every branch trivially unit-tested
/// (D-014's 100% line/region coverage gate applies to this crate too).
const REGISTRY: &[StdSymbol] = &[
    StdSymbol {
        module: StdModule::Math,
        name: "sqrt",
        kind: StdSymbolKind::Function {
            arg_tys: &[ScalarKind::Float],
            ret_ty: ScalarKind::Float,
        },
    },
    StdSymbol {
        module: StdModule::Math,
        name: "pi",
        kind: StdSymbolKind::Constant {
            ty: ScalarKind::Float,
        },
    },
];

/// Resolves a dotted-import module name (e.g. `"math"`) to a [`StdModule`],
/// or `None` if this pycc version does not recognize it (every other
/// stdlib/third-party module name, including every PEP-594 dead battery).
pub fn resolve_module(name: &str) -> Option<StdModule> {
    match name {
        "math" => Some(StdModule::Math),
        _ => None,
    }
}

/// Resolves a symbol name inside an already-bound module (e.g.
/// `resolve_symbol(StdModule::Math, "sqrt")`) to its registered
/// [`StdSymbol`], or `None` if the module is recognized but this particular
/// symbol is not registered (e.g. `math.tan`).
pub fn resolve_symbol(module: StdModule, name: &str) -> Option<StdSymbol> {
    REGISTRY
        .iter()
        .find(|sym| sym.module == module && sym.name == name)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_module_recognizes_math() {
        assert_eq!(resolve_module("math"), Some(StdModule::Math));
    }

    #[test]
    fn resolve_module_rejects_unregistered_name() {
        assert_eq!(resolve_module("os"), None);
        assert_eq!(resolve_module("sys"), None);
        assert_eq!(resolve_module("cgi"), None);
    }

    #[test]
    fn resolve_symbol_finds_math_sqrt() {
        let sym = resolve_symbol(StdModule::Math, "sqrt").expect("math.sqrt is registered");
        assert_eq!(sym.module, StdModule::Math);
        assert_eq!(sym.name, "sqrt");
        assert_eq!(
            sym.kind,
            StdSymbolKind::Function {
                arg_tys: &[ScalarKind::Float],
                ret_ty: ScalarKind::Float,
            }
        );
    }

    #[test]
    fn resolve_symbol_finds_math_pi() {
        let sym = resolve_symbol(StdModule::Math, "pi").expect("math.pi is registered");
        assert_eq!(sym.module, StdModule::Math);
        assert_eq!(sym.name, "pi");
        assert_eq!(
            sym.kind,
            StdSymbolKind::Constant {
                ty: ScalarKind::Float,
            }
        );
    }

    #[test]
    fn resolve_symbol_rejects_unregistered_symbol_in_registered_module() {
        assert_eq!(resolve_symbol(StdModule::Math, "tan"), None);
        assert_eq!(resolve_symbol(StdModule::Math, "floor"), None);
    }

    #[test]
    fn std_module_derives_are_exercised() {
        // Exercises `Debug`/`Clone`/`Copy`/`PartialEq`/`Eq` on `StdModule`
        // directly, since `resolve_module`'s equality checks above only
        // exercise `PartialEq` transitively.
        let m = StdModule::Math;
        let m2 = m;
        assert_eq!(format!("{m:?}"), "Math");
        assert_eq!(m, m2);
    }

    #[test]
    fn scalar_kind_and_symbol_derives_are_exercised() {
        let k = ScalarKind::Float;
        let k2 = k;
        assert_eq!(format!("{k:?}"), "Float");
        assert_eq!(k, k2);

        let sym = StdSymbol {
            module: StdModule::Math,
            name: "sqrt",
            kind: StdSymbolKind::Function {
                arg_tys: &[ScalarKind::Float],
                ret_ty: ScalarKind::Float,
            },
        };
        let sym2 = sym;
        assert_eq!(sym, sym2);
        assert!(format!("{sym:?}").contains("StdSymbol"));
    }
}
