import ast, importlib.util, os
EX = os.path.join(os.path.dirname(__file__), "..", "src", "lark", "examples", "json_parser.py")
def setup():
    tree = ast.parse(open(EX, encoding="utf-8").read())
    fn = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "test")
    text = next(s.value.value for s in fn.body if isinstance(s, ast.Assign) and s.targets[0].id == "test_json")
    spec = importlib.util.spec_from_file_location("json_parser", EX)
    m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
    parse = m.json_parser.parse
    def run():
        parse(text)
    return run
