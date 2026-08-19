class Vehicle:
    def __init__(self, wheels: int) -> None:
        self.wheels = wheels

    def describe(self) -> int:
        return self.wheels


class Car(Vehicle):
    def __init__(self, wheels: int, doors: int) -> None:
        super().__init__(wheels)
        self.doors = doors

    def describe(self) -> int:
        return super().describe() + self.doors


class SportsCar(Car):
    def __init__(self) -> None:
        super().__init__(4, 2)

    def describe(self) -> int:
        return super().describe() * 10


v = Vehicle(6)
print(v.describe())

c = Car(4, 4)
print(c.wheels)
print(c.doors)
print(c.describe())

s = SportsCar()
print(s.wheels)
print(s.doors)
print(s.describe())


# #587: `super().<name>` resolves the *class-level* members a CPython
# `super` object actually proxies along the MRO. A `@property` is such a
# member (a descriptor found on a class), so `super().power` calls the base
# class's getter rather than the subclass's override. An instance attribute
# established by `self.<attr> = ...` is not proxied and is rejected at
# compile time, so it cannot appear in a fixture that must match the oracle
# byte for byte.
class Engine:
    def __init__(self, power: int) -> None:
        self._power = power

    @property
    def power(self) -> int:
        return self._power


class TurboEngine(Engine):
    def __init__(self, power: int) -> None:
        super().__init__(power)

    @property
    def power(self) -> int:
        return self._power * 2

    def base_power(self) -> int:
        return super().power


t = TurboEngine(50)
print(t.power)
print(t.base_power())
