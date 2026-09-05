# PEP 563: `from __future__ import annotations` (superseded by PEP 649/749 on
# 3.14). pycc accepts the directive as a compile-time no-op (#919, D-229).
#
# What this fixture proves: the directive is accepted and the file runs
# byte-identically to CPython. What it does not exercise -- deliberately,
# because pycc rejects both today -- is PEP 563's distinguishing case, a
# forward reference to a name defined *later* in the module, and a string
# annotation; both are recorded `core` gaps for the row's flip.
from __future__ import annotations


class Node:
    def __init__(self, value: int) -> None:
        self.value = value

    def update(self, other: Node) -> None:
        self.value = other.value


def total(a: Node, b: Node) -> int:
    return a.value + b.value


a = Node(1)
b = Node(2)
a.update(b)
print(a.value)
print(total(a, b))
