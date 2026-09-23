import ast, json, sys
src = open(sys.argv[1], encoding="utf-8").read()
cases = []
for node in ast.walk(ast.parse(src)):
    if not (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)):
        continue
    if node.func.attr == "assertEqual":
        c = node.args[0]
        if isinstance(c, ast.Call) and isinstance(c.func, ast.Attribute) and c.func.attr in ("encode", "decode"):
            cases.append([c.func.attr, [ast.literal_eval(a) for a in c.args], {k.arg: ast.literal_eval(k.value) for k in c.keywords}, False, node.lineno, node.col_offset])
    elif node.func.attr == "assertRaises":
        f = node.args[1]
        if isinstance(f, ast.Attribute) and f.attr in ("encode", "decode"):
            cases.append([f.attr, [ast.literal_eval(a) for a in node.args[2:]], {k.arg: ast.literal_eval(k.value) for k in node.keywords}, True, node.lineno, node.col_offset])
cases.sort(key=lambda c: (c[4], c[5]))
json.dump([c[:4] for c in cases], open(sys.argv[2], "w", encoding="utf-8"))
print(len(cases))
