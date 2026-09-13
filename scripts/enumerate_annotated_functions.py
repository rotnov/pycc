#!/usr/bin/env python3
"""Derive the compile-unchanged denominator and its digest from a source tree.

`docs/TESTING.md`'s "The compile-unchanged count" bullet fixes the denominator
-- the number of annotated functions to be attempted -- and a digest over that
exact set **before** any compile may be scored, and it forbids publishing the
names themselves. A digest whose enumeration predicate and construction rule
are unstated is unreproducible at scoring time, so both are stated here and
committed, alongside the numbers, in
`scripts/bench_hosted_ext_precommit.json`.

The enumeration predicate, in full:

* every file whose name ends in `.py` under the given root, skipping any
  directory whose name begins with `.` or is one of `ENUMERATION_SKIP_DIRS`
  (virtual environments, caches and build output are not source);
* a file that does not parse is skipped rather than failing the walk, because
  a tree may legitimately carry fixtures for syntax errors;
* within a file, only **module-level** `def` statements qualify. A method, a
  nested function and an `async def` do not: an `ext` artifact exports
  module-level synchronous functions, so nothing else could be attempted;
* a `def` qualifies only when it is *fully* annotated -- a return annotation
  plus an annotation on every parameter, including positional-only,
  keyword-only, `*args` and `**kwargs`;
* the qualified name is the module's dotted path relative to the root, a `.`,
  and the function name. A `__init__.py` contributes its package's dotted path.

The script prints the count and the digest and nothing else: the tree it reads
may be proprietary, and only numbers are published (D-244 rule 6).
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
from pathlib import Path
import sys
import warnings

DIGEST_RULE = (
    "SHA-256 over the fully-qualified function names, sorted byte-wise "
    "ascending, joined by a single \\n, UTF-8, no trailing newline"
)

ENUMERATION_SKIP_DIRS = frozenset(
    {
        "__pycache__",
        "build",
        "dist",
        "node_modules",
        "site-packages",
        "venv",
    }
)


def _is_fully_annotated(node: ast.FunctionDef) -> bool:
    arguments = node.args
    if node.returns is None:
        return False
    positional = list(arguments.posonlyargs) + list(arguments.args) + list(arguments.kwonlyargs)
    for argument in positional:
        if argument.annotation is None:
            return False
    for optional in (arguments.vararg, arguments.kwarg):
        if optional is not None and optional.annotation is None:
            return False
    return True


def _module_path(root: Path, path: Path) -> str:
    relative = path.relative_to(root)
    parts = list(relative.parts)
    if parts[-1] == "__init__.py":
        parts = parts[:-1]
    else:
        parts[-1] = parts[-1][: -len(".py")]
    return ".".join(parts)


def _source_files(root: Path) -> list[Path]:
    found: list[Path] = []
    stack = [root]
    while stack:
        directory = stack.pop()
        for entry in sorted(directory.iterdir()):
            if entry.is_symlink():
                continue
            if entry.is_dir():
                if entry.name.startswith(".") or entry.name in ENUMERATION_SKIP_DIRS:
                    continue
                stack.append(entry)
            elif entry.is_file() and entry.name.endswith(".py"):
                found.append(entry)
    return found


def collect_annotated_functions(root: Path) -> list[str]:
    """Return the qualified names the predicate above selects, sorted."""

    names: list[str] = []
    for path in _source_files(root):
        try:
            with warnings.catch_warnings():
                # A tree that is not ours may carry escape-sequence warnings;
                # they are not this walk's business and must not reach stdout.
                warnings.simplefilter("ignore")
                tree = ast.parse(path.read_bytes())
        except (SyntaxError, ValueError):
            continue
        module = _module_path(root, path)
        prefix = f"{module}." if module else ""
        for node in tree.body:
            if isinstance(node, ast.FunctionDef) and _is_fully_annotated(node):
                names.append(f"{prefix}{node.name}")
    return sorted(names)


def digest_of(names: list[str]) -> str:
    """Apply `DIGEST_RULE` to an already-sorted set of qualified names."""

    joined = "\n".join(names)
    return hashlib.sha256(joined.encode("utf-8")).hexdigest()


def format_report(names: list[str]) -> str:
    """Render the only two facts that may be published."""

    return f"count={len(names)}\ndigest={digest_of(names)}"


def read_pre_registration(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("root", type=Path, help="source tree to enumerate")
    arguments = parser.parse_args(argv)
    root = arguments.root.resolve()
    if not root.is_dir():
        print("the given root is not a directory", file=sys.stderr)
        return 2
    print(format_report(collect_annotated_functions(root)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
