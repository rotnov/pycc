class Color(Enum):
    RED = 1
    GREEN = 2
    BLUE = 3

print(Color.RED.value)
print(Color.GREEN.value)
print(Color.BLUE.value)
print(Color.RED.name)
print(Color.GREEN.name)
print(Color.BLUE.name)

for c in Color:
    print(c.value)
    print(c.name)
