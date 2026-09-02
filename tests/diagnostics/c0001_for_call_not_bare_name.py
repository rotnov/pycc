d = {1: 2}


def f() -> int:
    for k in d.keys():
        return k
    return 0
