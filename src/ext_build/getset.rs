//! The read-only `Py_tp_getset` descriptors a constructible published class
//! carries (#1442): one per carriable instance-attribute slot and one per
//! carriable `@property`, so `instance.field` reads the compiled value on
//! the CPython side.
//!
//! **Why the host needs them.** D-258 makes an unannotated or `Any`/`object`
//! operand the opaque CPython object, and compiled code reads an attribute
//! of such an operand through `PyObject_GetAttr` (`pycc_ext_obj_getattr`).
//! Before #1442 a published type carried only `Py_tp_methods`, so
//! `other.state_stack` inside a compiled `__eq__(self, other)` raised
//! `AttributeError` for an `other` that really was a compiled instance --
//! the shape lark's `ParserState.__eq__` has. A getset descriptor is the
//! attribute the host's own lookup finds, so the same read now answers the
//! compiled field, and a host program reading `obj.field` gets it too.
//!
//! **Read-only.** No descriptor has a setter: a host-side store keeps
//! CPython's own `AttributeError` for a getset without one. Writing a slot
//! from the host is a separate contract (ownership of the replaced word,
//! the declared type of the slot) and is not part of this change.
//!
//! **Instance-typed fields (#1453).** A slot or property declared as a
//! regular class compiled in the same module -- lark's
//! `ParserState.parse_conf: ParseConf` -- is packed through the #1449
//! egress, `pycc_ext_pack_instance`, so the stored instance's live carrier
//! is returned again: `s.parse_conf is s.parse_conf`, and a host-constructed
//! instance reads back as the host's own object. The admitted classes are exactly the ones a
//! published signature may name ([`super::carrier_class_names`]): an enum
//! or exception-class field still gets no descriptor.
//!
//! **Silently partial.** A slot or property whose declared type the boundary
//! cannot pack from one machine word (a `list[int]` slot, an enum- or
//! exception-class-typed one, a `tuple`- or `memoryview`-returning getter)
//! simply gets no descriptor; it is never a `C0003`. The descriptors widen what a host can
//! observe of an object it already holds, and refusing a whole build because
//! one field cannot be observed would turn an additive capability into a
//! regression for every program that has such a field today. An optional
//! instance slot (`C | None`) never reaches this table: its annotation is
//! still refused at compile time with `T0049`.
//!
//! **Constructible classes only, for now.** The descriptors are part of
//! [`super::ExtCtor`], so only a constructible class's type object carries
//! them. Since #1435 every published type, and every on-demand carrier type
//! `pycc_ext_carrier_type` creates for a class that publishes nothing, also
//! wraps a live `inst`; those carry no table yet (#1448), so a field read
//! through one still raises `AttributeError`.

use std::collections::BTreeSet;

use pycc_hir::{HirClassDef, HirItem, HirModule, Ty, flat_attr_layout};

use super::export_name::ExtReceiver;
use super::{ExtCtor, ExtExport, carrier_class_names, inherited, namespace_owner, wrapper_for};

/// One read-only attribute a constructible published class exposes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ExtGetset {
    /// An instance-attribute slot, read straight out of the inner
    /// `PyInstanceObj` with the same checked accessor a compiled `self.x`
    /// read uses (#1388), so an unassigned slot raises the same
    /// `AttributeError`.
    Slot {
        /// The attribute name, which is the host-visible descriptor name.
        name: String,
        /// The slot index in `pycc_hir::flat_attr_layout` order -- the
        /// layout `ExtCtor::slot_names` allocates.
        index: usize,
        /// The slot's declared type, which picks the packer.
        ty: Ty,
    },
    /// A `@property`, read by calling its compiled getter through the same
    /// `METH_FASTCALL` wrapper an instance method gets.
    Property {
        /// The property name, which is the host-visible descriptor name.
        name: String,
        /// The zero-argument instance export the getter wrapper is rendered
        /// from. It is never in the export set ([`super::collect_exports`]
        /// excludes a getter as representation), so the wrapper is emitted
        /// here and nowhere else.
        getter: ExtExport,
    },
}

impl ExtGetset {
    /// The host-visible attribute name.
    pub(crate) fn name(&self) -> &str {
        match self {
            ExtGetset::Slot { name, .. } | ExtGetset::Property { name, .. } => name,
        }
    }
}

/// Whether a slot or getter of type `ty` gets a descriptor: the five types
/// whose value the boundary packs from one machine word, and (#1453) an
/// instance of one of `carrier_classes`, the module's regular classes,
/// whose word is the instance pointer #1449's egress packs.
fn carried(ty: &Ty, carrier_classes: &BTreeSet<String>) -> bool {
    match ty {
        Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Object => true,
        Ty::Instance(class) => carrier_classes.contains(class.as_str()),
        _ => false,
    }
}

/// The getset descriptors the constructible class `class` publishes, given
/// its definition and its MRO's definitions (most derived first): its
/// carriable slots in slot order, then its carriable properties in MRO
/// order.
///
/// A property is published only where it *wins the namespace walk*
/// ([`namespace_owner`]): a slot of the same name, or a nearer class's
/// member of that name, answers the attribute instead, exactly as it does
/// in Python and in [`super::collect_class_publications`].
pub(crate) fn collect_getsets(
    module: &HirModule,
    class_def: &HirClassDef,
    mro_defs: &[&HirClassDef],
) -> Vec<ExtGetset> {
    let class = class_def.name.as_str();
    let carrier_classes = carrier_class_names(module);
    let mut out: Vec<ExtGetset> = flat_attr_layout(mro_defs)
        .into_iter()
        .enumerate()
        .filter(|(_, (_, ty))| carried(ty, &carrier_classes))
        .map(|(index, (name, ty))| ExtGetset::Slot { name, index, ty })
        .collect();
    for def in mro_defs {
        for prop in &def.properties {
            if namespace_owner(module, &class_def.mro, &prop.name) != Some(def.name.as_str())
                || out.iter().any(|getset| getset.name() == prop.name)
            {
                continue;
            }
            let getter = inherited::receiver_exact_member(module, class, &prop.name, &prop.getter);
            if let Some(export) = getter_export(module, class, &prop.name, getter, &carrier_classes)
            {
                out.push(ExtGetset::Property {
                    name: prop.name.clone(),
                    getter: export,
                });
            }
        }
    }
    out
}

/// The zero-argument instance export for the compiled getter `getter`, or
/// `None` when its return type gets no descriptor. The *last* definition of
/// the name is the one codegen binds `fnptr_<name>` to, so it is the one
/// read.
fn getter_export(
    module: &HirModule,
    class: &str,
    prop: &str,
    getter: &str,
    carrier_classes: &BTreeSet<String>,
) -> Option<ExtExport> {
    let return_ty = module.items.iter().rev().find_map(|item| match item {
        HirItem::Function {
            name, return_ty, ..
        } if name == getter => Some(return_ty),
        _ => None,
    })?;
    carried(return_ty, carrier_classes).then(|| ExtExport {
        name: getter.to_string(),
        class: Some(class.to_string()),
        method: Some(prop.to_string()),
        returns_buffer_slice: false,
        receiver: ExtReceiver::SelfInstance,
        params: Vec::new(),
        param_writable: Vec::new(),
        defaults: Vec::new(),
        return_ty: return_ty.clone(),
    })
}

/// The C getter function name for `class`'s attribute `name`. Each part is
/// length-prefixed, so class `A_b` attribute `c` and class `A` attribute
/// `b_c` cannot collide.
fn getter_symbol(class: &str, name: &str) -> String {
    format!("pycc_ext_get_{}_{class}_{}_{name}", class.len(), name.len())
}

/// The packing statements for a slot word of type `ty`, already read into
/// the local `word`. The slot keeps its own reference, so the two packers
/// that discharge one -- `int` for a heap bigint (D-180 rule 6), `str` for
/// its `PyStrObj` -- are handed a reference taken here first; the object
/// packer takes its own new reference.
fn pack_slot_word(class: &str, name: &str, ty: &Ty) -> String {
    match ty {
        Ty::Int => format!(
            "    pycc_rt_bigint_retain(word);\n    \
             return pycc_ext_pack_int(\"{class}.{name}\", word);\n"
        ),
        Ty::Float => "    memcpy(&value, &word, sizeof value);\n    \
                      return pycc_ext_pack_float(value);\n"
            .to_string(),
        Ty::Bool => "    return pycc_ext_pack_bool((char)word);\n".to_string(),
        Ty::Str => "    pycc_rt_str_incref((void *)(intptr_t)word);\n    \
                    return pycc_ext_pack_str((void *)(intptr_t)word);\n"
            .to_string(),
        // #1453: the word is the stored instance's pointer. The instance is
        // never freed (D-107, D-154) and the packer takes no reference on
        // it, only a new one on the carrier it returns -- the live one when
        // the instance has one, so a repeated read is the same object.
        Ty::Instance(_) => {
            "    return pycc_ext_pack_instance((void *)(intptr_t)word);\n".to_string()
        }
        // `collect_getsets` admits exactly the `carried` types, so the
        // remaining one is the opaque object.
        _ => "    return pycc_ext_pack_object((void *)(intptr_t)word);\n".to_string(),
    }
}

/// The C text for one constructible class's descriptors: each property's
/// getter wrapper, each descriptor's getter function and the
/// `pycc_ext_type_getset_<Class>` table [`super::method_types_c`] installs
/// as `Py_tp_getset`. Empty when the class has no descriptor, in which case
/// no slot is installed and the class's generated C is unchanged.
///
/// `emitted` holds the compiled getters whose wrapper an earlier class
/// already rendered: a `Derived` that inherits `Base`'s `@property` with no
/// receiver-exact copy resolves to the same `Base.<name>` item, and a
/// second `pycc_ext_wrap_` definition of one symbol is a C redefinition
/// error, so each wrapper is rendered once per artifact.
pub(crate) fn getset_c(ctor: &ExtCtor, emitted: &mut Vec<String>) -> String {
    if ctor.getsets.is_empty() {
        return String::new();
    }
    let class = &ctor.class;
    let mut out = String::new();
    for getset in &ctor.getsets {
        let name = getset.name();
        let symbol = getter_symbol(class, name);
        match getset {
            ExtGetset::Slot { index, ty, .. } => {
                // A carrier `PyType_GenericNew` zeroed and `tp_init` never
                // filled (`mod.Class.__new__(mod.Class)`) has no instance, so
                // every slot of it is unassigned: the same `AttributeError`
                // the checked accessor raises for one unassigned slot.
                let float_local = if *ty == Ty::Float {
                    "    double value;\n"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "static PyObject *{symbol}(PyObject *self, void *closure)\n{{\n    \
                     void *inst = ((PyccExtInstance *)self)->inst;\n    long long word;\n\
                     {float_local}    (void)closure;\n    if (inst == NULL) {{\n        \
                     PyErr_SetString(PyExc_AttributeError, \
                     \"'{class}' object has no attribute '{name}'\");\n        \
                     return NULL;\n    }}\n    \
                     word = pycc_rt_instance_get_slot_checked(inst, {index});\n    \
                     if (pycc_rt_ext_pending_type() >= 0) {{\n        \
                     pycc_ext_raise_pending();\n        return NULL;\n    }}\n{}}}\n\n",
                    pack_slot_word(class, name, ty)
                ));
            }
            ExtGetset::Property { getter, .. } => {
                if !emitted.contains(&getter.name) {
                    emitted.push(getter.name.clone());
                    out.push_str(&wrapper_for(getter));
                }
                // The NULL-`inst` guard answers `AttributeError` before the
                // wrapper's own `TypeError` guard is reached, so `hasattr`
                // and `getattr(o, name, default)` on a carrier `tp_init`
                // never filled behave as they do for any missing attribute.
                out.push_str(&format!(
                    "static PyObject *{symbol}(PyObject *self, void *closure)\n{{\n    \
                     (void)closure;\n    if (((PyccExtInstance *)self)->inst == NULL) {{\n        \
                     PyErr_SetString(PyExc_AttributeError, \
                     \"'{class}' object has no attribute '{name}'\");\n        \
                     return NULL;\n    }}\n    \
                     return pycc_ext_wrap_{wrapped}(self, NULL, 0);\n}}\n\n",
                    wrapped = pycc_codegen::mangle_ext_name(&getter.name)
                ));
            }
        }
    }
    out.push_str(&format!(
        "static PyGetSetDef pycc_ext_type_getset_{class}[] = {{\n"
    ));
    for getset in &ctor.getsets {
        let name = getset.name();
        out.push_str(&format!(
            "    {{\"{name}\", {}, NULL, NULL, NULL}},\n",
            getter_symbol(class, name)
        ));
    }
    out.push_str("    {NULL, NULL, NULL, NULL, NULL},\n};\n\n");
    out
}
