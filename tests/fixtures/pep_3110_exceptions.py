def risky(kind: int) -> int:
    if kind == 0:
        raise ValueError("value")
    if kind == 1:
        raise KeyError("key")
    return kind


def classify(kind: int) -> str:
    result = "unset"
    try:
        risky(kind)
    except ValueError:
        result = "value"
    except KeyError:
        result = "key"
    else:
        result = "ok"
    finally:
        print("cleanup")
    return result


def reraise(kind: int) -> str:
    try:
        risky(kind)
    except ValueError:
        print("inner handler")
        raise
    return "no exception"


print(classify(0))
print(classify(1))
print(classify(2))

print(reraise(2))

try:
    print(reraise(0))
except ValueError:
    print("outer handler")

try:
    print("body runs")
finally:
    print("finally always runs")
