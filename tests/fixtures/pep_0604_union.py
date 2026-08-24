# PEP 604 -- Allow writing union types as X | Y (D-197, #763, Part 1 of #747).
#
# Scope proven here: `Optional[int]` (`int | None`, either operand order),
# `is`/`is not` as a presence test against `None`, and both a module-level
# and a function-local `int | None` variable. Flow-sensitive narrowing
# (reading the wrapped payload back as a plain `int` inside a `not None`
# branch) is out of scope for this PR -- see D-197 and issue #747 -- so
# every check below reads only the boolean presence result, never an
# unwrapped payload.

present: int | None = 5
absent: int | None = None
print(present is None)
print(present is not None)
print(absent is None)
print(absent is not None)

# Reversed operand order: `None | T` must parse identically to `T | None`.
reversed_order: None | int = 7
print(reversed_order is None)


def maybe_double(x: int, present: bool) -> int | None:
    if present:
        return x * 2
    return None


doubled = maybe_double(21, True)
skipped = maybe_double(21, False)
print(doubled is None)
print(skipped is None)


def local_optional() -> None:
    y: int | None = 9
    print(y is not None)
    z: int | None = None
    print(z is not None)


local_optional()
