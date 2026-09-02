import os
class A:
    x: int = 1
class B(A):
    def __init__(self) -> None:
        self.v = 1
def g(a: A) -> int:
    return 1
def h(*args: int) -> int:
    return 0
