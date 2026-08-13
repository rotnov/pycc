# PEP 557 -- Dataclasses (pycc fixture)
#
# This fixture exercises pycc's dataclass implementation:
#   - `@dataclass` with required annotated fields
#   - Auto-generated `__init__` (constructor)
#   - Auto-generated `__eq__` (equality) and `__ne__` (inequality)
#   - Auto-generated `__repr__` (string representation)
#   - Dataclass inheritance (field merging: parent fields first)
#
# Field defaults (`field(default=...)`, `field(default_factory=...)`) are
# not supported in this version -- only required fields are supported.

@dataclass
class Point:
    x: int
    y: int

# Auto-generated constructor: Point(x, y)
p1 = Point(1, 2)
p2 = Point(1, 2)
p3 = Point(3, 4)

# Auto-generated __eq__ and __ne__
print(p1 == p2)
print(p1 != p3)
print(p1 == p3)

# Auto-generated __repr__
print(p1)
print(p3)

# Dataclass inheritance: parent fields come first
@dataclass
class NamedPoint(Point):
    name: int

np = NamedPoint(10, 20, 30)
print(np.x)
print(np.y)
print(np.name)
print(np)

# Equality with inherited fields
np2 = NamedPoint(10, 20, 30)
np3 = NamedPoint(10, 20, 99)
print(np == np2)
print(np != np3)
