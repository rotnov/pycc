class Boxed:
    def __init__(self) -> None:
        self.tag = 0

    @staticmethod
    def __class_getitem__(key: int) -> int:
        return key * 2


class Named:
    def __init__(self) -> None:
        self.tag = 1

    @classmethod
    def __class_getitem__(cls, key: int) -> int:
        return key + 1000


class Base:
    def __init__(self) -> None:
        self.tag = 2

    @staticmethod
    def __class_getitem__(key: int) -> int:
        return key + 100


class Derived(Base):
    def __init__(self) -> None:
        self.tag = 3


def index_via_hook(key: int) -> int:
    return Boxed[key]


print(Boxed[3])
print(Boxed[0])
print(Named[7])
print(Derived[5])
print(index_via_hook(21))

boxed = Boxed()
print(boxed.tag)
