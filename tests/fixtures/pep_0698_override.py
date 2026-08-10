class Base:
    def __init__(self) -> None:
        self.x = 0

    def f(self) -> int:
        return 1

class Derived(Base):
    def __init__(self) -> None:
        return

    @override
    def f(self) -> int:
        return 2

d = Derived()
print(d.f())
