# PEP 3129 -- Class Decorators (pycc fixture)
#
# This fixture exercises pycc's narrow class-decorator support:
#   - `@dataclass` (PEP 557) is recognized and triggers auto-generated
#     `__init__`, `__eq__`, and `__repr__`.
#   - `@dataclass_transform()` (PEP 681) is recognized and treated as
#     equivalent to `@dataclass`.
#   - Any other class decorator is rejected with C0001.
#   - `@dataclass(frozen=True)` and other option-bearing forms are
#     rejected with C0001.
#
# Only the `@dataclass` and `@dataclass_transform()` forms are executable
# in this fixture; the rejected forms are documented below as comments.

@dataclass
class Point:
    x: int
    y: int

p = Point(3, 4)
print(p.x)
print(p.y)
print(p)

# PEP 681: `@dataclass_transform()` is equivalent to `@dataclass`.
@dataclass_transform()
class Tagged:
    tag: int
    value: int

t = Tagged(1, 42)
print(t)

# Rejected forms (not executed -- documented for conformance reference):
#
#   @dataclass(frozen=True)
#   class Frozen:
#       x: int
#
#   @some_other_decorator
#   class Other:
#       x: int
#
# Both produce C0001 ("not supported yet") in pycc.
