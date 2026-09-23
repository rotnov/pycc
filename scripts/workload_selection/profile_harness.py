"""Profile one #1207 candidate under the section-5 conditions.

usage: python profile_harness.py <driver.py> <import_root_name> <out.json>
The driver module defines setup() -> run, where run() executes the full
documented case list once. The harness calls run() K=10 times per profiled
run, after one unprofiled warm-up pass, 3 profiled runs.
"""
import ast, cProfile, importlib.util, json, os, pstats, sys, sysconfig

K = 10
RUNS = 3

def load(path):
    spec = importlib.util.spec_from_file_location("driver", path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m

_loop_cache = {}
def has_statement_loop(filename, lineno, name):
    key = (filename, lineno, name)
    if key in _loop_cache:
        return _loop_cache[key]
    res = None
    try:
        tree = ast.parse(open(filename, encoding="utf-8").read())
    except Exception:
        _loop_cache[key] = None
        return None
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == name and node.lineno <= lineno <= (node.body[0].lineno if node.body else node.lineno):
            res = False
            stack = list(node.body)
            while stack:
                n = stack.pop()
                if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda, ast.ClassDef)):
                    continue
                if isinstance(n, (ast.For, ast.AsyncFor, ast.While)):
                    res = True
                    break
                stack.extend(ast.iter_child_nodes(n))
            break
    _loop_cache[key] = res
    return res

def main():
    driver_path, root_name, out = sys.argv[1:4]
    purelib = sysconfig.get_paths()["purelib"]
    own_root = os.path.realpath(os.path.join(purelib, root_name)) + os.sep
    stdlib = os.path.realpath(sysconfig.get_paths()["stdlib"]) + os.sep
    drv = load(driver_path)
    run = drv.setup()
    run()  # warm-up, unprofiled
    runs = []
    for r in range(RUNS):
        pr = cProfile.Profile()
        pr.enable()
        for _ in range(K):
            run()
        pr.disable()
        st = pstats.Stats(pr)
        total = sum(v[2] for v in st.stats.values())
        entries = []
        for (fn, ln, nm), v in st.stats.items():
            rfn = os.path.realpath(fn) if not fn.startswith("~") else fn
            if fn.startswith("~") or not fn.endswith(".py"):
                cat = "C"
                loop = None
            elif rfn.startswith(own_root):
                loop = has_statement_loop(rfn, ln, nm)
                cat = "own-Python-with-statement-loop" if loop else "own-Python-without"
            elif rfn.startswith(purelib):
                cat, loop = "dependency Python", None
            elif rfn.startswith(stdlib):
                cat, loop = "stdlib Python", None
            else:
                cat, loop = "driver/other Python", None
            entries.append({"key": f"{rfn}:{ln}:{nm}", "tottime": v[2], "share": v[2] / total, "cat": cat})
        entries.sort(key=lambda e: -e["tottime"])
        runs.append({"total": total, "top": entries[:15],
                     "qualifying": {e["key"]: e["share"] for e in entries if e["cat"] == "own-Python-with-statement-loop" and e["share"] >= 0.20}})
    keys = set(runs[0]["qualifying"])
    for r in runs[1:]:
        keys &= set(r["qualifying"])
    json.dump({"runs": runs, "qualifying_all_runs": sorted(keys)}, open(out, "w"), indent=1)
    for i, r in enumerate(runs):
        t = r["top"][0]
        print(f"run{i+1}: total={r['total']:.3f}s rank1 cat={t['cat']} share={t['share']:.3f}")
    print("qualifying in all runs:", len(keys))

main()
