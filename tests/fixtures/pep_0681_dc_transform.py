# PEP 681 -- dataclass_transform (pycc fixture)
#
# This fixture exercises pycc's `@dataclass_transform()` support:
#   - `@dataclass_transform()` is treated as equivalent to `@dataclass`
#   - Auto-generated `__init__`, `__eq__`, `__repr__` work identically
#   - The keyword arguments (`eq_default`, `order_default`, etc.) are
#     accepted but ignored -- pycc always generates the standard set.

@dataclass_transform()
class Color:
    r: int
    g: int
    b: int

c1 = Color(255, 128, 0)
c2 = Color(255, 128, 0)
c3 = Color(0, 0, 0)

print(c1)
print(c1 == c2)
print(c1 != c3)

# Inheritance with dataclass_transform
@dataclass_transform()
class AlphaColor(Color):
    a: int

ac = AlphaColor(255, 128, 0, 64)
print(ac)
print(ac.a)
