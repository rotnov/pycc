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
