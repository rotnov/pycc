//! The read-only `Py_tp_getset` descriptors a constructible published class
//! carries (#1442): which slots and properties get one, and the generated
//! C each renders to. Lowered from real source through the `--ext`
//! frontend, so the slot types are the ones D-258 really assigns (an `Any`
//! or object-carrying container field is the opaque object).

use super::*;

/// The constructors -- and so the descriptors -- of `source` built as an
/// `--ext` module, plus the generated companion.
fn ext_build(tag: &str, source: &str) -> (Vec<ExtCtor>, String) {
    let dir = pycc_scratch::ScratchDir::new(tag).expect("scratch");
    let src = dir.join("m.py");
    std::fs::write(&src, source).expect("write source");
    let module = crate::frontend::resolve_frontend_with(
        &src,
        Some("m"),
        crate::modules::RelativeImports::Project,
    )
    .unwrap_or_else(|_| panic!("the fixture must type-check in an ext build: {source}"));
    let exports = collect_exports(&module).expect("the module exports");
    let publications = collect_class_publications(&module, &exports);
    let ctors = collect_constructors(&module, &publications);
    let inc = generate_exports_inc("m", &exports, &[], &publications, &ctors);
    (ctors, inc)
}

fn getsets_of<'c>(ctors: &'c [ExtCtor], class: &str) -> &'c [ExtGetset] {
    &ctors
        .iter()
        .find(|ctor| ctor.class == class)
        .unwrap_or_else(|| panic!("`{class}` is constructible"))
        .getsets
}

fn slot(name: &str, index: usize, ty: Ty) -> ExtGetset {
    ExtGetset::Slot {
        name: name.to_string(),
        index,
        ty,
    }
}

const SCALARS: &str = "from typing import Any, List\n\
    class P:\n\
    \x20   xs: List[int]\n\
    \x20   def __init__(self, n: int, r: float, b: bool, s: str, o: Any) -> None:\n\
    \x20       self.n = n\n\
    \x20       self.r = r\n\
    \x20       self.xs = [1]\n\
    \x20       self.b = b\n\
    \x20       self.s = s\n\
    \x20       self.o = o\n\
    \x20   def get(self) -> int:\n\
    \x20       return self.n\n";

/// Every slot whose word the boundary packs gets a descriptor at its own
/// slot index; a `list[int]` slot, which no packer carries, is skipped
/// without a `C0003` and without disturbing the later slots' indices.
#[test]
fn every_carriable_slot_gets_a_descriptor_at_its_slot_index() {
    let (ctors, _) = ext_build("1442_scalars", SCALARS);
    assert_eq!(
        getsets_of(&ctors, "P"),
        &[
            slot("n", 0, Ty::Int),
            slot("r", 1, Ty::Float),
            slot("b", 3, Ty::Bool),
            slot("s", 4, Ty::Str),
            slot("o", 5, Ty::Object),
        ]
    );
}

/// Each slot type is packed by the packer its word needs, the two packers
/// that discharge a reference are handed one taken first, and the table is
/// installed as `Py_tp_getset` with no setter.
#[test]
fn each_slot_getter_packs_its_word_and_the_table_is_installed() {
    let (_, inc) = ext_build("1442_scalars_c", SCALARS);
    let getter = |name: &str, index: usize, pack: &str| {
        format!(
            "static PyObject *pycc_ext_get_1_P_1_{name}(PyObject *self, void *closure)\n{{\n    \
             void *inst = ((PyccExtInstance *)self)->inst;\n    long long word;\n{local}    \
             (void)closure;\n    if (inst == NULL) {{\n        \
             PyErr_SetString(PyExc_AttributeError, \"'P' object has no attribute '{name}'\");\n        \
             return NULL;\n    }}\n    word = pycc_rt_instance_get_slot_checked(inst, {index});\n    \
             if (pycc_rt_ext_pending_type() >= 0) {{\n        pycc_ext_raise_pending();\n        \
             return NULL;\n    }}\n{pack}}}\n\n",
            local = if name == "r" {
                "    double value;\n"
            } else {
                ""
            },
        )
    };
    for expected in [
        getter(
            "n",
            0,
            "    pycc_rt_bigint_retain(word);\n    return pycc_ext_pack_int(\"P.n\", word);\n",
        ),
        getter(
            "r",
            1,
            "    memcpy(&value, &word, sizeof value);\n    return pycc_ext_pack_float(value);\n",
        ),
        getter("b", 3, "    return pycc_ext_pack_bool((char)word);\n"),
        getter(
            "s",
            4,
            "    pycc_rt_str_incref((void *)(intptr_t)word);\n    \
             return pycc_ext_pack_str((void *)(intptr_t)word);\n",
        ),
        getter(
            "o",
            5,
            "    return pycc_ext_pack_object((void *)(intptr_t)word);\n",
        ),
    ] {
        assert!(inc.contains(&expected), "missing:\n{expected}\nin:\n{inc}");
    }
    assert!(
        inc.contains(
            "static PyGetSetDef pycc_ext_type_getset_P[] = {\n    \
             {\"n\", pycc_ext_get_1_P_1_n, NULL, NULL, NULL},\n    \
             {\"r\", pycc_ext_get_1_P_1_r, NULL, NULL, NULL},\n    \
             {\"b\", pycc_ext_get_1_P_1_b, NULL, NULL, NULL},\n    \
             {\"s\", pycc_ext_get_1_P_1_s, NULL, NULL, NULL},\n    \
             {\"o\", pycc_ext_get_1_P_1_o, NULL, NULL, NULL},\n    \
             {NULL, NULL, NULL, NULL, NULL},\n};\n"
        ),
        "{inc}"
    );
    assert!(
        inc.contains(
            "    {Py_tp_dealloc, pycc_ext_instance_dealloc},\n    \
             {Py_tp_getset, pycc_ext_type_getset_P},\n    {0, NULL},\n"
        ),
        "{inc}"
    );
    assert!(!inc.contains("pycc_ext_get_1_P_2_xs"), "{inc}");
}

/// A class with no carriable slot and no property installs no table, so
/// its slot array is exactly what it was before #1442.
#[test]
fn a_class_with_nothing_to_describe_installs_no_getset_slot() {
    let (ctors, inc) = ext_build(
        "1442_none",
        "from typing import List\n\
         class Q:\n\
         \x20   xs: List[int]\n\
         \x20   def __init__(self) -> None:\n\
         \x20       self.xs = [1]\n\
         \x20   def first(self) -> int:\n\
         \x20       return self.xs[0]\n",
    );
    assert!(getsets_of(&ctors, "Q").is_empty());
    assert!(!inc.contains("Py_tp_getset"), "{inc}");
    assert!(!inc.contains("PyGetSetDef"), "{inc}");
}

const PROPERTIES: &str = "from typing import Any, List\n\
    class Base:\n\
    \x20   def __init__(self, stack: Any) -> None:\n\
    \x20       self.stack = stack\n\
    \x20   @property\n\
    \x20   def top(self) -> Any:\n\
    \x20       return self.stack[-1]\n\
    \x20   @property\n\
    \x20   def shadowed(self) -> int:\n\
    \x20       return 1\n\
    \x20   @property\n\
    \x20   def items(self) -> List[int]:\n\
    \x20       return [1]\n\
    \x20   def size(self) -> int:\n\
    \x20       return len(self.stack)\n\
    class Derived(Base):\n\
    \x20   def shadowed(self) -> int:\n\
    \x20       return 2\n";

/// A property is a descriptor where it wins the namespace walk: inherited
/// by `Derived` unchanged, but shadowed there by an ordinary method, and
/// skipped -- not a `C0003` -- when its return type has no packer.
#[test]
fn a_property_is_described_where_it_wins_the_namespace_walk() {
    let (ctors, _) = ext_build("1442_properties", PROPERTIES);
    let names = |class: &str| {
        getsets_of(&ctors, class)
            .iter()
            .map(|getset| getset.name().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(names("Base"), ["stack", "top", "shadowed"]);
    assert_eq!(names("Derived"), ["stack", "top"]);
    let ExtGetset::Property { getter, .. } = &getsets_of(&ctors, "Derived")[1] else {
        panic!("`top` is a property descriptor");
    };
    assert_eq!(getter.name, "Base.top");
    assert_eq!(getter.class.as_deref(), Some("Derived"));
    assert_eq!(getter.receiver, ExtReceiver::SelfInstance);
    assert!(getter.params.is_empty());
    assert_eq!(getter.return_ty, Ty::Object);
}

/// Two classes reading one inherited getter share one wrapper -- a second
/// definition of the same `pycc_ext_wrap_` symbol is a C redefinition
/// error -- while each class keeps its own NULL-guarded getter shim.
#[test]
fn an_inherited_property_s_wrapper_is_rendered_once() {
    let (_, inc) = ext_build("1442_properties_c", PROPERTIES);
    let wrapper = "static PyObject *pycc_ext_wrap_0m4_Base3_top(PyObject *self, \
                   PyObject *const *args, Py_ssize_t nargs)\n";
    assert_eq!(inc.matches(wrapper).count(), 1, "{inc}");
    for class in ["Base", "Derived"] {
        let shim = format!(
            "static PyObject *pycc_ext_get_{len}_{class}_3_top(PyObject *self, void *closure)\n{{\n    \
             (void)closure;\n    if (((PyccExtInstance *)self)->inst == NULL) {{\n        \
             PyErr_SetString(PyExc_AttributeError, \"'{class}' object has no attribute 'top'\");\n        \
             return NULL;\n    }}\n    return pycc_ext_wrap_0m4_Base3_top(self, NULL, 0);\n}}\n\n",
            len = class.len()
        );
        assert!(inc.contains(&shim), "missing:\n{shim}\nin:\n{inc}");
    }
}

/// A getter whose body depends on its receiver is compiled once more for
/// the subclass (#1337, D-254), and the subclass's descriptor reads that
/// copy rather than the base's compilation.
#[test]
fn a_subclass_s_property_descriptor_reads_its_receiver_exact_copy() {
    let (ctors, inc) = ext_build(
        "1442_copy",
        "class A:\n\
         \x20   def __init__(self) -> None:\n\
         \x20       self.k = 0\n\
         \x20   def m(self) -> int:\n\
         \x20       return 1\n\
         \x20   @property\n\
         \x20   def p(self) -> int:\n\
         \x20       return self.m()\n\
         class B(A):\n\
         \x20   def m(self) -> int:\n\
         \x20       return 2\n",
    );
    let getter_of = |class: &str| {
        getsets_of(&ctors, class)
            .iter()
            .find_map(|getset| match getset {
                ExtGetset::Property { getter, .. } => Some(getter.name.clone()),
                ExtGetset::Slot { .. } => None,
            })
            .expect("`p` is a property descriptor")
    };
    assert_eq!(getter_of("A"), "A.p");
    assert_eq!(getter_of("B"), "B.p");
    assert!(
        inc.contains("return pycc_ext_wrap_0m1_B1_p(self, NULL, 0);"),
        "{inc}"
    );
}
