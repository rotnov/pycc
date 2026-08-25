# PEP 604 -- Allow writing union types as X | Y (D-197, #763, Part 1 of #747).
#
# Scope proven here: `Optional[int]` (`int | None`, either operand order),
# `is`/`is not` as a presence test against `None`, and both a module-level
# and a function-local `int | None` variable. Flow-sensitive narrowing
# (reading the wrapped payload back as a plain `int` inside a `not None`
# branch) is now in scope too (D-201, #769, Part 2 of #747), restricted to
# a top-level `if name is None:` / `if name is not None:` test -- the
# rungs below exercise both polarities, the early-return-narrows-the-
# continuation shape, kill-on-reassignment inside a narrowed branch, and a
# function-local narrowed read, alongside the original presence-only
# checks this fixture already proved.

present: int | None = 5
absent: int | None = None
print(present is None)
print(present is not None)
print(absent is None)
print(absent is not None)

# D-201, #769: `is not None` narrows the body -- `present + 1` reads
# `present` as a plain `int`, not `Optional[int]`.
if present is not None:
    print(present + 1)

# D-201, #769: `is None` narrows the `orelse` -- the mirror polarity.
if present is None:
    pass
else:
    print(present * 2)

# D-201, #769: a reassignment inside the narrowed branch kills the
# narrowing for every read after it, within the same branch -- `present`
# is written back as a bare `int` here, then the branch's own remaining
# reads still work (through the ordinary `Optional`-widening `Assign`
# path, not narrowing).
if present is not None:
    present = present + 100
    print(present is None)
    print(present is not None)

# Reversed operand order: `None | T` must parse identically to `T | None`,
# and narrows exactly the same way.
reversed_order: None | int = 7
print(reversed_order is None)
if reversed_order is not None:
    print(reversed_order + 1)


def maybe_double(x: int, present: bool) -> int | None:
    if present:
        return x * 2
    return None


doubled = maybe_double(21, True)
skipped = maybe_double(21, False)
print(doubled is None)
print(skipped is None)


def describe(x: int | None) -> None:
    # D-201, #769: the early-return continuation shape -- a guard clause
    # whose body definitely terminates (ends in a bare `return`) narrows
    # every read after it in the same statement sequence, with no `else`
    # needed at all.
    if x is None:
        print("absent")
        return
    print(x + 1)


describe(doubled)
describe(skipped)


def local_optional() -> None:
    y: int | None = 9
    print(y is not None)
    z: int | None = None
    print(z is not None)
    # D-201, #769: narrowing works identically inside a function body.
    if y is not None:
        print(y + 1)


local_optional()
