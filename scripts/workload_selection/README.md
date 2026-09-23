# Workload-selection instruments (#1207)

These files are the instruments that ran the replacement-workload selection
pre-registered in [#1207](https://github.com/rotnov/pycc/issues/1207) on
2026-09-23. The outcome is recorded in
[the #1207 result comment](https://github.com/rotnov/pycc/issues/1207#issuecomment-5791727061)
and summarized in `docs/TESTING.md`, in the "Status" subsection of the hosted
`ext` benchmark protocol.

They are committed byte for byte as they ran and nothing has been fixed. They
are the record of that run, not a reusable instrument. No CI job or test runs
them.

| File | What it is |
|---|---|
| `pins.txt` | The frozen pool, one candidate per line: PyPI name, repository URL, the commit the pinned release tag resolves to, and the import root relative to the checkout. |
| `req-idna.txt`, `req-lark.txt` | Hash-pinned requirement lines for the two wheels that were installed, `idna` 3.20 and `lark` 1.3.1. |
| `profile_harness.py` | The #1207 section-5 profiler. It runs one unprofiled warm-up, then 3 `cProfile` runs of the driver's case list repeated K = 10 times. It classifies each entry against D-247's four clauses and writes a JSON summary. |
| `drivers/gen_idna.py` | Extracts the `encode`/`decode` cases from `idna`'s `tests/test_idna_uts46.py` into `inputs/idna.json`. |
| `drivers/drv_idna.py` | The `idna` driver. It replays those cases and catches the expected `IDNAError`. |
| `drivers/drv_lark.py` | The `lark` driver. It builds the parser exactly as `examples/json_parser.py` does and parses that file's `test_json` string. |

## Known deviations, left unfixed

The result comment discloses two deviations. Neither affected either outcome.

- **Decorated functions.** `has_statement_loop` matches the profiler's line
  number against a `def` line. cProfile reports a decorated function at its
  first decorator's line, so the lookup misses that function and the entry is
  classified as "own-Python-without".
- **Warm-up placement.** The harness runs one warm-up before the first profiled
  run. Section 5 asks for one before each run.

Before these files are used for any further selection, the lookup must accept
the decorator line and must fail closed on a miss, as the result comment
requires. The warm-up must also move inside the run loop. Both changes are
deliberately left out of this commit, which records what actually ran.

## Reproducing the run

The drivers resolve `../inputs` and `../src` relative to `drivers/`. Copy this
directory somewhere outside the repository first, so that the checkouts, the
venvs and the outputs do not land in the working tree:

```sh
cp -R scripts/workload_selection /some/scratch/w1207
cd /some/scratch/w1207

# Interpreter: the uv / python-build-standalone CPython 3.14.7.
uv python install 3.14.7
PY="$(uv python find 3.14.7)"
"$PY" -VV
"$PY" -c "import sysconfig; print(sysconfig.get_config_var('CONFIG_ARGS'))"

# Clone each candidate at its pinned commit into src/<name>.
while read -r name url commit root; do
  git clone --quiet "$url" "src/$name"
  git -C "src/$name" checkout --quiet "$commit"
done < pins.txt

# Step 0: the static C3 count over each import root. Run this from the pycc
# checkout:
#   python3 scripts/enumerate_annotated_functions.py /some/scratch/w1207/src/<name>/<root>

# The idna input.
mkdir -p inputs
python3 drivers/gen_idna.py src/idna/tests/test_idna_uts46.py inputs/idna.json

# One fresh venv per candidate, then the profile.
for name in idna lark; do
  uv venv --python "$PY" "v-$name"
  uv pip install --python "v-$name/bin/python" \
    --require-hashes --only-binary :all: --no-deps -r "req-$name.txt"
  "v-$name/bin/python" profile_harness.py "drivers/drv_$name.py" "$name" "out-$name.json"
done
```

The second argument to `profile_harness.py` is the installed package's
directory name under `site-packages`, which is `idna` or `lark` here. The
`out-*.json` summaries hold per-function names and stay local, as the result
comment states.
