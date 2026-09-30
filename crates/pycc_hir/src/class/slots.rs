//! `__slots__` in a class body (#1368).
//!
//! pycc already fixes an instance's layout at compile time from `__init__`
//! (D-154), and a pycc instance seen from a CPython host already has no
//! `__dict__`. So admitting `__slots__` is not a layout change: it is
//! accepting the binding and checking it statically against the rules
//! CPython's `type.__new__` applies while the `class` statement executes,
//! plus the one runtime rule a slotted instance adds (a store to an
//! undeclared attribute raises `AttributeError`).
//!
//! [`super::body`] skips a `__slots__` binding in a non-`@dataclass` body
//! ([`is_slots_binding`]); `module::lower_top_level_item` then calls
//! [`check_class`] for every lowered class, which
//!
//! 1. refuses a layout conflict between the class's bases
//!    ([`check_layout`]), which CPython reports before it looks at the
//!    class's own namespace;
//! 2. parses the binding ([`own_slots`]): a string, or a tuple or list of
//!    strings, each an identifier;
//! 3. refuses a slot that collides with a name in the class's own namespace
//!    ([`check_namespace_conflicts`]);
//! 4. refuses every dunder-named slot ([`check_dunder_slots`]);
//! 5. refuses a slot whose name an ancestor binds at class level, which the
//!    slot's member descriptor would shadow ([`check_inherited_class_names`]);
//! 6. when every class in the MRO binds `__slots__` and none is an exception
//!    class, refuses an instance attribute `__init__` stores outside the
//!    union of the MRO's slot lists ([`check_undeclared_stores`]).
//!
//! The per-class answer is kept in the module's side table
//! (`LoweredModule::class_slots`): one row per user class, `None` for a
//! class that binds no `__slots__` (its instances keep a `__dict__` in
//! CPython), `Some(list)` for one that does -- `Some(vec![])` is
//! `__slots__ = ()`. Every user class has a row, so an MRO entry without one
//! is a seeded builtin exception class, which the lookups assert; a hole in
//! the table's threading therefore panics in a test instead of reading as
//! "has a `__dict__`" and silently admitting a program CPython rejects.
//!
//! Names are compared after CPython's private-name mangling ([`mangle`]): a
//! slot entry is mangled with the declaring class's name, and an instance
//! attribute with the storing class's name.

use super::HirClassDef;
use crate::exception::is_builtin_exception_class;
use crate::unsupported;
use pycc_ast::{Expr, Number, Stmt, StmtClassDef};
use pycc_diag::{Diagnostic, Span};

/// One row of a module's `__slots__` side table: the class name, and its
/// own slot list if it binds `__slots__` (see the module docs).
pub type ClassSlotsRow = (String, Option<Vec<String>>);

/// Slot names whose treatment differs between the CPython versions pycc
/// supports, so no single answer is right for all of them: 3.11 accepts all
/// three (and 3.9 is derived to behave the same way), 3.13 raises the
/// class-variable `ValueError` for all three, and 3.14 raises it for the
/// first two and accepts `__annotations__`.
const VERSION_SENSITIVE_SLOT_NAMES: [&str; 3] = [
    "__firstlineno__",
    "__static_attributes__",
    "__annotations__",
];

/// Whether `stmt` is a class-body `__slots__` binding this module owns: the
/// bare `__slots__ = <v>` (a single target), or the annotated
/// `__slots__: T = <v>`. A value-less `__slots__: T` is only an annotation
/// -- CPython binds nothing and keeps the instance `__dict__` -- and stays
/// with `super::reserved_names`.
pub(super) fn is_slots_binding(stmt: &Stmt) -> bool {
    slots_binding_value(stmt).is_some()
}

/// The value of a `__slots__` binding (see [`is_slots_binding`]).
fn slots_binding_value(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Assign(assign) => (assign.targets.len() == 1 && is_slots_name(&assign.targets[0]))
            .then_some(assign.value.as_ref()),
        Stmt::AnnAssign(ann) => ann.value.as_deref().filter(|_| is_slots_name(&ann.target)),
        _ => None,
    }
}

fn is_slots_name(target: &Expr) -> bool {
    matches!(target, Expr::Name(name) if name.id.as_str() == "__slots__")
}

/// Checks a lowered class's `__slots__` against CPython's class-creation
/// rules and returns its row for the module's side table.
///
/// `class_slots` holds a row for every user class lowered (or imported)
/// before this one; `defined_classes` is the matching class table.
pub(crate) fn check_class(
    def: &StmtClassDef,
    class_def: &HirClassDef,
    defined_classes: &[(String, HirClassDef)],
    class_slots: &[ClassSlotsRow],
) -> Result<Option<Vec<String>>, Diagnostic> {
    let ancestors = Ancestors {
        defined_classes,
        class_slots,
    };
    check_layout(def, class_def, &ancestors)?;
    let Some((slots, binding_range)) = own_slots(def)? else {
        return Ok(None);
    };
    check_namespace_conflicts(def, &slots)?;
    check_dunder_slots(&slots, binding_range)?;
    check_inherited_class_names(def, class_def, &slots, binding_range, &ancestors)?;
    check_undeclared_stores(def, class_def, &slots, binding_range, &ancestors)?;
    Ok(Some(slots))
}

/// The class tables an ancestor lookup reads.
struct Ancestors<'a> {
    defined_classes: &'a [(String, HirClassDef)],
    class_slots: &'a [ClassSlotsRow],
}

impl Ancestors<'_> {
    /// The side-table row of an ancestor, or `None` for a seeded builtin
    /// exception class -- the only kind of class table entry without one.
    fn row(&self, name: &str) -> Option<&Option<Vec<String>>> {
        let row = self
            .class_slots
            .iter()
            .find(|(class_name, _)| class_name == name)
            .map(|(_, slots)| slots);
        assert!(row.is_some() || is_builtin_exception_class(name));
        row
    }

    /// An ancestor's MRO. Every MRO entry of a lowered class was itself
    /// lowered (or seeded) earlier, so the lookup cannot miss.
    fn mro(&self, name: &str) -> &[String] {
        &self
            .defined_classes
            .iter()
            .find(|(class_name, _)| class_name == name)
            .expect("every class in an MRO is in the class table")
            .1
            .mro
    }

    /// Whether `name`'s MRO contains a builtin exception class, i.e. whether
    /// it is an exception class (the MRO is transitive, so a user exception
    /// deriving from another user exception counts too).
    fn is_exception_class(&self, name: &str) -> bool {
        self.mro(name).iter().any(|entry| self.row(entry).is_none())
    }

    /// Whether user class `name` binds `mangled` at class level: a class
    /// variable or any other class-body binding
    /// ([`super::declares_name_outside_class_attrs`]). The HIR keeps names
    /// as written, so each source spelling that mangles (by `name`) to
    /// `mangled` is looked up: `mangled` itself, and the private `__x` that
    /// `_<name>__x` came from.
    fn binds_at_class_level(&self, name: &str, mangled: &str) -> bool {
        let class_def = self
            .defined_classes
            .iter()
            .find(|(class_name, _)| class_name == name)
            .map(|(_, class_def)| class_def)
            .expect("every class in an MRO is in the class table");
        let prefix = format!("_{}", name.trim_start_matches('_'));
        let private = mangled
            .strip_prefix(prefix.as_str())
            .filter(|rest| rest.starts_with("__"));
        [Some(mangled), private]
            .into_iter()
            .flatten()
            .filter(|spelling| mangle(spelling, name) == mangled)
            .any(|spelling| {
                class_def
                    .class_attrs
                    .iter()
                    .any(|(bound, _, _)| bound == spelling)
                    || super::declares_name_outside_class_attrs(class_def, spelling)
            })
    }
}

/// CPython's private-name mangling: `__x` (no trailing `__`) in class `C`
/// becomes `_C__x`, with the class name's leading underscores stripped; a
/// class whose name is all underscores mangles nothing.
fn mangle(name: &str, class_name: &str) -> String {
    let stripped = class_name.trim_start_matches('_');
    if !name.starts_with("__") || name.ends_with("__") || stripped.is_empty() {
        return name.to_string();
    }
    format!("_{stripped}{name}")
}

/// Refuses a class whose bases CPython cannot lay out together.
///
/// `mro::validate_mro_slot_layout` (#969) has already refused two bases
/// whose *stored* attributes are disjoint; this covers what it admits and
/// CPython rejects with `TypeError: multiple bases have instance lay-out
/// conflict`: the ancestors with a non-empty own slot list, plus one node
/// for `BaseException` when the MRO holds an exception class, must form a
/// chain under the ancestor relation. The class's own slots cannot
/// conflict (it descends from every ancestor), so only the MRO after it is
/// considered -- which is also why this runs before the class's own
/// namespace is looked at, as in CPython.
///
/// Builtin exception layouts differ among themselves (`OSError` has its
/// own), so when a slotted exception class is an ancestor, every builtin
/// exception class in the MRO must be on that class's own ancestor chain.
/// That is stricter than CPython, which accepts `class G(E, KeyError)` for
/// a slotted `E(ValueError)`, and is refused as not supported yet.
fn check_layout(
    def: &StmtClassDef,
    class_def: &HirClassDef,
    ancestors: &Ancestors<'_>,
) -> Result<(), Diagnostic> {
    let ancestry = &class_def.mro[1..];
    let slotted: Vec<&String> = ancestry
        .iter()
        .filter(|name| {
            ancestors
                .row(name)
                .is_some_and(|slots| slots.as_ref().is_some_and(|list| !list.is_empty()))
        })
        .collect();
    let has_exception = ancestry.iter().any(|name| ancestors.row(name).is_none());
    let related = |a: &str, b: &str| ancestors.mro(a).iter().any(|entry| entry == b);
    for (index, first) in slotted.iter().enumerate() {
        let conflicts_with_sibling = slotted[index + 1..]
            .iter()
            .any(|second| !related(first, second) && !related(second, first));
        if conflicts_with_sibling || (has_exception && !ancestors.is_exception_class(first)) {
            return Err(unsupported(
                format!(
                    "class `{}` cannot be created: its bases have incompatible `__slots__` \
                     layouts -- `{first}` declares slots and is not on the same inheritance \
                     chain as another slotted or exception base, so CPython raises \
                     `TypeError: multiple bases have instance lay-out conflict`",
                    class_def.name
                ),
                def.range,
            ));
        }
        if has_exception
            && ancestry
                .iter()
                .any(|name| ancestors.row(name).is_none() && !ancestors.mro(first).contains(name))
        {
            return Err(unsupported(
                format!(
                    "class `{}` combines the slotted exception class `{first}` with a builtin \
                     exception base outside `{first}`'s own ancestry; this is not supported yet \
                     -- builtin exception classes have different instance layouts, and pycc does \
                     not model which of them CPython can combine with `__slots__`",
                    class_def.name
                ),
                def.range,
            ));
        }
    }
    Ok(())
}

/// Parses the class's own `__slots__` binding, returning its entries and
/// the binding's range, or `None` when the body binds no `__slots__`.
fn own_slots(def: &StmtClassDef) -> Result<Option<(Vec<String>, Span)>, Diagnostic> {
    let mut bindings = def
        .body
        .iter()
        .filter_map(|stmt| slots_binding_value(stmt).map(|value| (stmt, value)));
    let Some((stmt, value)) = bindings.next() else {
        return Ok(None);
    };
    if let Some((second, _)) = bindings.next() {
        return Err(unsupported(
            "a second `__slots__` binding in one class body is not supported yet",
            pycc_ast::stmt_range(second),
        ));
    }
    let range = pycc_ast::stmt_range(stmt);
    let items: Vec<&Expr> = match value {
        Expr::StringLiteral(_) => vec![value],
        Expr::Tuple(tuple) => tuple.elts.iter().collect(),
        Expr::List(list) => list.elts.iter().collect(),
        other => {
            if let Some(type_name) = literal_type_name(other) {
                return Err(unsupported(
                    format!(
                        "`__slots__` must be a string or an iterable of strings -- CPython \
                         raises `TypeError: '{type_name}' object is not iterable` when the class \
                         is created"
                    ),
                    range,
                ));
            }
            return Err(unsupported(
                format!(
                    "a `__slots__` value that is not a string literal or a tuple or list of \
                     string literals is not supported yet (found {})",
                    pycc_ast::expr_kind_name(other)
                ),
                range,
            ));
        }
    };
    let mut slots = Vec::with_capacity(items.len());
    for item in items {
        slots.push(slot_entry(item, range.clone())?);
    }
    Ok(Some((slots, Span::new(range.start, range.end))))
}

/// Validates one `__slots__` entry in CPython's order -- a string, then an
/// identifier -- and then refuses the entries pycc does not support.
fn slot_entry(item: &Expr, range: std::ops::Range<u32>) -> Result<String, Diagnostic> {
    let Expr::StringLiteral(literal) = item else {
        if let Some(type_name) = literal_type_name(item) {
            return Err(unsupported(
                format!(
                    "every `__slots__` entry must be a string -- CPython raises `TypeError: \
                     __slots__ items must be strings, not '{type_name}'` when the class is \
                     created"
                ),
                range,
            ));
        }
        return Err(unsupported(
            format!(
                "a `__slots__` entry that is not a string literal is not supported yet (found \
                 {})",
                pycc_ast::expr_kind_name(item)
            ),
            range,
        ));
    };
    let entry = literal.value.to_str().to_string();
    if !entry.is_ascii() {
        return Err(unsupported(
            format!("the non-ASCII `__slots__` entry `{entry}` is not supported yet"),
            range,
        ));
    }
    if !is_ascii_identifier(&entry) {
        return Err(unsupported(
            format!(
                "the `__slots__` entry {entry:?} is not an identifier -- CPython raises \
                 `TypeError: __slots__ must be identifiers` when the class is created"
            ),
            range,
        ));
    }
    if entry == "__dict__" || entry == "__weakref__" {
        return Err(unsupported(
            format!(
                "a `{entry}` entry in `__slots__` is not supported yet -- pycc instances have \
                 neither a `__dict__` nor weak-reference support"
            ),
            range,
        ));
    }
    if VERSION_SENSITIVE_SLOT_NAMES.contains(&entry.as_str()) {
        return Err(unsupported(
            format!(
                "a `__slots__` entry named `{entry}` is not supported yet -- whether it \
                 conflicts with the class namespace differs between CPython versions"
            ),
            range,
        ));
    }
    Ok(entry)
}

/// The Python type name of a literal that is not a string, for CPython's
/// `TypeError` text, or `None` for anything else.
fn literal_type_name(expr: &Expr) -> Option<&'static str> {
    match expr {
        Expr::NumberLiteral(number) => Some(match number.value {
            Number::Int(_) => "int",
            Number::Float(_) => "float",
            Number::Complex { .. } => "complex",
        }),
        Expr::BooleanLiteral(_) => Some("bool"),
        Expr::NoneLiteral(_) => Some("NoneType"),
        _ => None,
    }
}

/// `str.isidentifier()` restricted to ASCII (a non-ASCII entry is refused
/// before this is asked).
fn is_ascii_identifier(entry: &str) -> bool {
    let mut chars = entry.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Refuses a slot whose (mangled) name is also bound in the class's own
/// namespace, which CPython reports as `ValueError: '<x>' in __slots__
/// conflicts with class variable`.
///
/// The namespace is what the class body binds -- every `def` (methods,
/// properties, static and class methods, `__init__`) and every assigned
/// name (including a D-256 foreign static) -- plus `__doc__` when the body
/// opens with a docstring, and the implicit `__module__` and `__slots__`.
/// Value-less annotations bind nothing; `__qualname__`, `__classcell__` and
/// an ancestor's class-level names are not in the dictionary CPython checks
/// (an inherited name is refused separately, by
/// [`check_inherited_class_names`]).
fn check_namespace_conflicts(def: &StmtClassDef, slots: &[String]) -> Result<(), Diagnostic> {
    let class_name = def.name.as_str();
    let mut namespace: Vec<String> = vec!["__module__".to_string(), "__slots__".to_string()];
    if let Some(Stmt::Expr(first)) = def.body.first()
        && matches!(*first.value, Expr::StringLiteral(_))
    {
        namespace.push("__doc__".to_string());
    }
    for stmt in &def.body {
        match stmt {
            Stmt::FunctionDef(method) => namespace.push(method.name.to_string()),
            Stmt::Assign(assign) => namespace.extend(assign.targets.iter().filter_map(bound_name)),
            Stmt::AnnAssign(ann) if ann.value.is_some() => {
                namespace.extend(bound_name(&ann.target));
            }
            _ => {}
        }
    }
    let namespace: Vec<String> = namespace
        .iter()
        .map(|name| mangle(name, class_name))
        .collect();
    for slot in slots {
        let mangled = mangle(slot, class_name);
        if namespace.contains(&mangled) {
            return Err(unsupported(
                format!(
                    "the `__slots__` entry `{slot}` of class `{class_name}` is also bound in the \
                     class body -- CPython raises `ValueError: '{mangled}' in __slots__ \
                     conflicts with class variable` when the class is created"
                ),
                def.range,
            ));
        }
    }
    Ok(())
}

/// Refuses every dunder-named slot (`__hash__`, `__eq__`, `__len__`, ...).
/// CPython gives many `__x__` names a special meaning, and a slot of such a
/// name changes the class's behaviour (a `__hash__` slot makes the class
/// unhashable); pycc does not model which names do, so every dunder entry is
/// refused. It runs after the namespace check, so a dunder slot that is also
/// bound in the body keeps CPython's own `ValueError`; a private `__x` name
/// is not a dunder and stays admitted.
fn check_dunder_slots(slots: &[String], binding_range: Span) -> Result<(), Diagnostic> {
    let Some(slot) = slots.iter().find(|slot| is_dunder(slot)) else {
        return Ok(());
    };
    Err(unsupported(
        format!(
            "a `__slots__` entry named `{slot}` is not supported yet -- CPython gives many \
             `__x__` names a special meaning (a `__hash__` slot makes the class unhashable), \
             and pycc does not model which, so every dunder entry is refused"
        ),
        binding_range.start..binding_range.end,
    ))
}

/// The non-dunder class-level names of the builtin exception classes pycc
/// seeds (`BaseException`'s, `OSError`'s and `BaseExceptionGroup`'s), from
/// `dir()` on CPython 3.14; 3.9 lacks `add_note` and the group names, which
/// only makes the list conservative there.
const BUILTIN_EXCEPTION_CLASS_NAMES: [&str; 13] = [
    "add_note",
    "args",
    "characters_written",
    "derive",
    "errno",
    "exceptions",
    "filename",
    "filename2",
    "message",
    "split",
    "strerror",
    "subgroup",
    "with_traceback",
];

/// Refuses a slot whose (mangled) name an ancestor binds at class level --
/// a class variable, a method, a static or class method, a property, or a
/// builtin exception class's attribute (`args`, `with_traceback`, ...).
/// CPython accepts the class, but the slot's member descriptor in the
/// class's own dictionary shadows the inherited `B.<name>`, so reading the
/// never-assigned slot raises `AttributeError` where pycc would find the
/// inherited binding. An ancestor's instance attribute is no class-level
/// binding and does not conflict, and neither does an ancestor's own slot
/// of the same name (re-declaring a base slot): an ancestor cannot both
/// slot a name and bind it at class level, since [`check_namespace_conflicts`]
/// refused that ancestor already.
fn check_inherited_class_names(
    def: &StmtClassDef,
    class_def: &HirClassDef,
    slots: &[String],
    binding_range: Span,
    ancestors: &Ancestors<'_>,
) -> Result<(), Diagnostic> {
    let class_name = def.name.as_str();
    for slot in slots {
        let mangled = mangle(slot, class_name);
        for ancestor in class_def.mro.iter().skip(1) {
            let binds = if ancestors.row(ancestor).is_none() {
                BUILTIN_EXCEPTION_CLASS_NAMES.contains(&mangled.as_str())
            } else {
                ancestors.binds_at_class_level(ancestor, &mangled)
            };
            if binds {
                return Err(unsupported(
                    format!(
                        "the `__slots__` entry `{slot}` of class `{class_name}` is not supported \
                         yet -- `{ancestor}` binds `{mangled}` at class level, and CPython's \
                         member descriptor for the slot shadows the inherited \
                         `{ancestor}.{mangled}`, so reading the unset slot raises \
                         `AttributeError` where pycc would find the inherited binding"
                    ),
                    binding_range.start..binding_range.end,
                ));
            }
        }
    }
    Ok(())
}

/// Whether `name` is a dunder: `__x__` with a non-empty `x`.
fn is_dunder(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

fn bound_name(target: &Expr) -> Option<String> {
    target.as_name_expr().map(|name| name.id.to_string())
}

/// Refuses an instance attribute store CPython would reject because the
/// instance has no `__dict__`: when every class in the MRO binds `__slots__`
/// and none is an exception class, each of the class's own instance
/// attributes (`HirClassDef::attrs`, every one stored at the top level of
/// `__init__`) must be in the union of the MRO's slot lists.
///
/// A base's own attributes were checked when the base was lowered. An MRO
/// with any class that binds no `__slots__`, or with an exception class,
/// keeps a `__dict__` in CPython, so nothing is checked there.
///
/// `typing.Generic` also defines `__slots__ = ()`; it is refused as a base
/// today (#886), and when it is admitted it must count as slotted-empty here,
/// or a generic slotted class would lose this check.
fn check_undeclared_stores(
    def: &StmtClassDef,
    class_def: &HirClassDef,
    own: &[String],
    binding_range: Span,
    ancestors: &Ancestors<'_>,
) -> Result<(), Diagnostic> {
    let class_name = class_def.name.as_str();
    let mut declared: Vec<String> = own.iter().map(|slot| mangle(slot, class_name)).collect();
    for ancestor in &class_def.mro[1..] {
        let Some(Some(slots)) = ancestors.row(ancestor) else {
            return Ok(());
        };
        declared.extend(slots.iter().map(|slot| mangle(slot, ancestor)));
    }
    for (attr, _) in &class_def.attrs {
        let mangled = mangle(attr, class_name);
        if declared.contains(&mangled) {
            continue;
        }
        let span = init_store_span(def, attr).unwrap_or(binding_range);
        return Err(Diagnostic::error(
            "T0044",
            format!(
                "class `{class_name}` has no slot for attribute `{attr}` -- every class in its \
                 MRO binds `__slots__`, so its instances have no `__dict__`, and CPython \
                 raises `AttributeError: '{class_name}' object has no attribute '{mangled}' \
                 and no __dict__ for setting new attributes` (3.13+ wording) at this store"
            ),
            span,
        ));
    }
    Ok(())
}

/// The span of the first top-level `<receiver>.<attr>` store target in the
/// class's own `__init__`, if one is found.
fn init_store_span(def: &StmtClassDef, attr: &str) -> Option<Span> {
    let init = def.body.iter().find_map(|stmt| match stmt {
        Stmt::FunctionDef(method) if method.name.as_str() == "__init__" => Some(method),
        _ => None,
    })?;
    let receiver = init.parameters.args.first()?.parameter.name.as_str();
    let is_store = |target: &Expr| {
        matches!(target, Expr::Attribute(attribute)
            if attribute.attr.as_str() == attr
                && matches!(attribute.value.as_ref(), Expr::Name(name) if name.id.as_str() == receiver))
    };
    init.body.iter().find_map(|stmt| {
        let target = match stmt {
            Stmt::Assign(assign) => assign.targets.iter().find(|target| is_store(target))?,
            Stmt::AnnAssign(ann) if is_store(&ann.target) => ann.target.as_ref(),
            _ => return None,
        };
        let range = pycc_ast::expr_range(target);
        Some(Span::new(range.start, range.end))
    })
}

#[cfg(test)]
#[path = "slots_tests.rs"]
mod tests;
