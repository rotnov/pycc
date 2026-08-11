use std::io::Write;
use std::process::Command;

fn pycc_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_pycc"))
}

fn write_fixture(dir: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(source.as_bytes()).unwrap();
    path
}

/// #433: `super().__init__()` with no arguments — the simplest case.
/// `B.__init__` calls `A.__init__` via `super()`, which sets `self.x = 1`.
/// The attribute is then readable on the `B` instance.
#[test]
fn super_init_no_args_builds_and_runs() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_init_no_args_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "init_no_args.py",
        "class A:\n    def __init__(self) -> None:\n        self.x = 1\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\nb = B()\nprint(b.x)\n",
    );
    let out = dir.join("init_no_args");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "pycc build should succeed for super().__init__() with no args"
    );

    let output = Command::new(&out).output().unwrap();
    assert_eq!(
        output.stdout, b"1\n",
        "super().__init__() should initialize the base class attribute"
    );
}

/// #433: `super().__init__(x)` with arguments — the base class's `__init__`
/// takes a parameter, and `super().__init__(x)` passes it through.
#[test]
fn super_init_with_args_builds_and_runs() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_init_args_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "init_args.py",
        "class A:\n    def __init__(self, x: int) -> None:\n        self.x = x\nclass B(A):\n    def __init__(self, x: int, y: int) -> None:\n        super().__init__(x)\n        self.y = y\nb = B(10, 20)\nprint(b.x)\nprint(b.y)\n",
    );
    let out = dir.join("init_args");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "pycc build should succeed for super().__init__(x) with args"
    );

    let output = Command::new(&out).output().unwrap();
    assert_eq!(
        output.stdout, b"10\n20\n",
        "super().__init__(x) should pass the argument to the base class"
    );
}

/// #433: `super().method()` — calling a base class method via `super()`.
/// The base method operates on `self` (the most-derived instance), so
/// attributes set by the derived class's `__init__` are visible.
#[test]
fn super_method_call_builds_and_runs() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_method_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "method.py",
        "class A:\n    def __init__(self) -> None:\n        self.val = 42\n    def get_val(self) -> int:\n        return self.val\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n    def get_val_via_super(self) -> int:\n        return super().get_val()\nb = B()\nprint(b.get_val_via_super())\n",
    );
    let out = dir.join("method");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "pycc build should succeed for super().method()"
    );

    let output = Command::new(&out).output().unwrap();
    assert_eq!(
        output.stdout, b"42\n",
        "super().method() should call the base class method on self"
    );
}

/// #433: a 3-level inheritance chain with `super()` at each level.
/// `C.__init__` calls `super().__init__()` → `B.__init__` → `super().__init__()`
/// → `A.__init__`. Each level sets an attribute. `C.describe()` calls
/// `super().describe()` → `B.describe()` → `super().describe()` →
/// `A.describe()`, building up a sum.
#[test]
fn three_level_super_chain_builds_and_runs() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_chain_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "chain.py",
        "class A:\n    def __init__(self) -> None:\n        self.val = 1\n    def describe(self) -> int:\n        return self.val\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n        self.val = 2\n    def describe(self) -> int:\n        return super().describe() + 10\nclass C(B):\n    def __init__(self) -> None:\n        super().__init__()\n        self.val = 3\n    def describe(self) -> int:\n        return super().describe() + 100\nc = C()\nprint(c.describe())\n",
    );
    let out = dir.join("chain");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "pycc build should succeed for a 3-level super() chain"
    );

    let output = Command::new(&out).output().unwrap();
    // C.describe() → B.describe() + 100 → A.describe() + 10 + 100 → 3 + 10 + 100 = 113
    assert_eq!(
        output.stdout, b"113\n",
        "3-level super() chain should build up the sum correctly"
    );
}

/// #433: `super().attr` — reading a base class attribute via `super()`.
/// The attribute is resolved starting from the next class in the MRO,
/// and the slot index is from the full MRO's flat layout.
#[test]
fn super_attr_read_builds_and_runs() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_attr_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "attr.py",
        "class A:\n    def __init__(self) -> None:\n        self.x = 99\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n    def get_x_via_super(self) -> int:\n        return super().x\nb = B()\nprint(b.get_x_via_super())\n",
    );
    let out = dir.join("attr");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "pycc build should succeed for super().attr"
    );

    let output = Command::new(&out).output().unwrap();
    assert_eq!(
        output.stdout, b"99\n",
        "super().attr should read the base class attribute from self"
    );
}

/// #433: `super()` outside a method body is rejected with C0001 at
/// HIR-lowering time (build and check).
#[test]
fn super_outside_method_is_a_build_error() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_outside_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "outside.py",
        "x = super()\n",
    );
    let out = dir.join("outside");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "pycc build should fail for super() outside a method body"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("C0001"),
        "should report C0001 for bare super(), got: {stderr}"
    );
}

/// #433: `super()` with arguments is rejected — only zero-arg `super()`
/// (PEP 3135) is supported.
#[test]
fn super_with_arguments_is_a_build_error() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_args_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "super_args.py",
        "class A:\n    def __init__(self) -> None:\n        return\nclass B(A):\n    def __init__(self) -> None:\n        super(A, self).__init__()\n",
    );
    let out = dir.join("super_args");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "pycc build should fail for super() with arguments"
    );
    // super(A, self) is not a zero-arg super() call, so it falls through
    // to the known-but-unsupported builtin path (C0001).
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("C0001"),
        "should report C0001 for super() with arguments, got: {stderr}"
    );
}

/// #433: `super().method()` where the method doesn't exist in any base
/// class is rejected with T0044 at type-check time.
#[test]
fn super_method_not_in_base_is_a_check_error() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_missing_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "missing.py",
        "class A:\n    def __init__(self) -> None:\n        return\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n    def call_missing(self) -> int:\n        return super().nonexistent()\n",
    );
    let out = dir.join("missing");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "pycc build should fail for super().nonexistent()"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("T0044"),
        "should report T0044 for super().nonexistent(), got: {stderr}"
    );
}

/// #433: `super().__init__(wrong_type)` — type mismatch in the arguments
/// passed to the base class's `__init__` is rejected at type-check time.
#[test]
fn super_init_type_mismatch_is_a_check_error() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_typemismatch_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "type_mismatch.py",
        "class A:\n    def __init__(self, x: int) -> None:\n        self.x = x\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__(\"hello\")\n",
    );
    let out = dir.join("type_mismatch");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "pycc build should fail for super().__init__() with wrong argument type"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("T0021"),
        "should report T0021 for type mismatch, got: {stderr}"
    );
}

/// #433: `super().__init__()` with wrong arity — too many or too few
/// arguments for the base class's `__init__` is rejected.
#[test]
fn super_init_wrong_arity_is_a_check_error() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_arity_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "arity.py",
        "class A:\n    def __init__(self, x: int) -> None:\n        self.x = x\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n",
    );
    let out = dir.join("arity");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "pycc build should fail for super().__init__() with wrong arity"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("T0021"),
        "should report T0021 for wrong arity, got: {stderr}"
    );
}

/// #433: `super().method()` with arguments — the method takes parameters
/// and `super().method(args)` passes them through correctly.
#[test]
fn super_method_with_args_builds_and_runs() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_method_args_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "method_args.py",
        "class A:\n    def __init__(self) -> None:\n        self.base = 100\n    def combine(self, n: int) -> int:\n        return self.base + n\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n    def combine_via_super(self, n: int) -> int:\n        return super().combine(n)\nb = B()\nprint(b.combine_via_super(5))\n",
    );
    let out = dir.join("method_args");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "pycc build should succeed for super().method(args)"
    );

    let output = Command::new(&out).output().unwrap();
    assert_eq!(
        output.stdout, b"105\n",
        "super().method(args) should pass arguments to the base method"
    );
}

/// #433: `super().method(undefined_var)` — the argument fails type
/// inference (unbound name), which is propagated by the `?` in the
/// super().method() type-checking path before `resolve_super_method_call`
/// is ever called.
#[test]
fn super_method_with_unbound_arg_is_a_check_error() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_unbound_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "unbound.py",
        "class A:\n    def __init__(self, x: int) -> None:\n        self.x = x\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__(undefined_var)\n",
    );
    let out = dir.join("unbound");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "pycc build should fail for super().__init__() with an unbound argument"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("T0021"),
        "should report T0021 for unbound argument, got: {stderr}"
    );
}

/// #433: `super()` in a class with no base class — `super()` has no
/// next class in the MRO, so any `super().method()` or `super().attr`
/// should be rejected with T0044 (no such member in any base class).
#[test]
fn super_in_class_with_no_base_is_a_check_error() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_433_nobase_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "no_base.py",
        "class A:\n    def __init__(self) -> None:\n        super().__init__()\n",
    );
    let out = dir.join("no_base");

    let output = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "pycc build should fail for super() in a class with no base"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("T0044"),
        "should report T0044 for super() with no base class, got: {stderr}"
    );
}

/// #433 review fix: super().attr where the attribute is redeclared with a
/// different type in a derived class must be rejected, because the shared
/// slot will contain the redeclared type at runtime.
#[test]
fn super_attr_with_redeclared_type_is_a_check_error() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_issue433_redecl_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "redecl.py",
        "class A:\n    def __init__(self) -> None:\n        self.x = 1\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n        self.x = \"hello\"\n    def get_base_x(self) -> int:\n        return super().x\n",
    );

    let output = Command::new(pycc_bin())
        .args(["check", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "pycc check should fail for super().attr with redeclared type"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("T0021") && stdout.contains("redeclared"),
        "should report T0021 with redeclared message, got: {stdout}"
    );
}

/// #433 review: super().attr where the attribute is redeclared with the
/// SAME type in a derived class should still work (no false positive).
#[test]
fn super_attr_with_same_type_redeclaration_builds_and_runs() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_issue433_same_redecl_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = write_fixture(
        &dir,
        "same_redecl.py",
        "class A:\n    def __init__(self) -> None:\n        self.x = 1\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n        self.x = 2\n    def get_base_x(self) -> int:\n        return super().x\nb = B()\nprint(b.get_base_x())\n",
    );
    let out = dir.join("same_redecl");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "pycc build should succeed for super().attr with same-type redeclaration"
    );

    let output = Command::new(&out).output().unwrap();
    // super().x reads the same shared slot as self.x. Since B's __init__
    // wrote 2 to the slot (after calling super().__init__() which wrote 1),
    // the value is 2. This is correct for a shared-slot layout — super().x
    // and self.x read the same memory.
    assert_eq!(
        output.stdout, b"2\n",
        "super().x reads the same shared slot as self.x, so it returns the last-written value (2)"
    );
}

/// #433 review: diamond inheritance with super() — documents the static
/// dispatch limitation. B.f() calls super().f() which resolves to A.f()
/// (not C.f()), because super() skips to the next class in B's own MRO.
#[test]
fn super_in_diamond_skips_sibling_classes() {
    let dir = std::env::temp_dir()
        .join(format!("pycc_issue433_diamond_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // MRO for D is [D, B, C, A]. B.f calls super().f → resolves to A.f
    // (B's own MRO is [B, A], so B's super_mro is [A], and A.f is found).
    let src = write_fixture(
        &dir,
        "diamond.py",
        "class A:\n    def __init__(self) -> None:\n        return\n    def f(self) -> str:\n        return \"A\"\nclass B(A):\n    def __init__(self) -> None:\n        super().__init__()\n    def f(self) -> str:\n        return \"B->\" + super().f()\nclass C(A):\n    def __init__(self) -> None:\n        super().__init__()\n    def f(self) -> str:\n        return \"C\"\nclass D(B, C):\n    def __init__(self) -> None:\n        super().__init__()\n    def f(self) -> str:\n        return \"D->\" + super().f()\nd = D()\nprint(d.f())\n",
    );
    let out = dir.join("diamond");

    let status = Command::new(pycc_bin())
        .args(["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(
        status.success(),
        "pycc build should succeed for diamond inheritance with super()"
    );

    let output = Command::new(&out).output().unwrap();
    // D.f calls super().f → D's super_mro is [B, C, A]. B has f, so B.f.
    // B.f calls super().f → B's super_mro is [A] (B's own MRO is [B, A],
    // not [B, C, A] — C is only in D's MRO, not B's). A has f, so A.f.
    // Result: "D->B->A"
    // This documents the static dispatch limitation: B's super() resolves
    // based on B's own MRO, not the derived class's MRO. In CPython with
    // dynamic dispatch, super() in B would see D's MRO and call C.f.
    assert_eq!(
        output.stdout, b"D->B->A\n",
        "diamond super() uses the defining class's own MRO, not the derived class's MRO"
    );
}
