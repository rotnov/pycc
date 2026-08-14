#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Arena: one task x every installed harness x two conditions (with / without the patch).

No eyeballing: models are pinned, metrics are machine-collected, and a verdict
is printed only when the data can carry one. Runs execute inside a Docker
container by default — the host leaked its settings, its global skills and its
system-python packages into runs, each measured before it was believed.

Usage: uv run arena.py <fixture-dir> [--runs N] [--sandbox auto|docker|host]
       uv run arena.py --docker-login claude   # one-time, claude auth lives in the Keychain
"""
import argparse
import difflib
import hashlib
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

CLAUDE_MODEL = os.environ.get("ARENA_CLAUDE_MODEL", "sonnet")
DEVIN_MODEL = os.environ.get("ARENA_DEVIN_MODEL", "glm-5-2")
CODEX_MODEL = os.environ.get("ARENA_CODEX_MODEL", "gpt-5.6-luna")
GROK_MODEL = os.environ.get("ARENA_GROK_MODEL", "grok-4.5")
EFFORT = os.environ.get("ARENA_EFFORT", "low")
# The judge is instrumentation, not a contestant: it runs on the host through
# the claude CLI and never enters the container.
JUDGE_MODEL = os.environ.get("ARENA_JUDGE_MODEL", "sonnet")
# Judgements are independent CLI calls at ~60-90s each; sequentially, 24 runs
# cost half an hour of pure waiting. Four at a time keeps clear of rate limits.
JUDGE_PARALLEL = int(os.environ.get("ARENA_JUDGE_PARALLEL", "4"))

HARNESSES = ("claude", "devin", "codex", "grok")
REQUIRED = ("task.md", "verify.py", "control.md", "patch.md")

# Rows and env are JSON-shaped: heterogeneous by nature, so the value type is
# deliberately loose. Aliases keep that decision in one place instead of
# scattering `dict[str, Any]` through every signature.
Row = dict[str, Any]
Env = dict[str, Any]
Stats = dict[str, Any]

MIN_TRACE_FIELDS = 2      # a trace line is "<tag>\t<tool>[\t<arg>]"
STUCK_THRESHOLD = 3       # failures on one target before it counts as stuck
MIN_RUNS_FOR_VERDICT = 3  # fewer than this cannot carry a verdict: behaviour is stochastic
HARNESS_TIMEOUT_S = 1800
SETUP_TIMEOUT_S = 300
JUDGE_TIMEOUT_S = 240
SNAP_LIMIT = 200_000      # bytes; larger files are diffed by hash, not by content
DIFF_FOR_JUDGE = 8_000    # chars of diff the judge sees; the full diff is on disk
TAIL_FOR_JUDGE = 2_500    # chars of transcript tail the judge sees
TREE_FOR_JUDGE = 60       # files listed for the judge; the tree is the fact-check
TRACE_FOR_JUDGE = 40      # tool calls listed for the judge
CONTENT_FOR_JUDGE = 6_000  # chars per changed file shown in full to the judge
CHANGED_FOR_JUDGE = 5      # changed files shown in full
JUDGE_MAX_SCORE = 10

# ── docker ─────────────────────────────────────────────────────────────────
# One image, one named volume per harness mounted at /root. The volume carries
# auth and caches between runs and nothing else — no operator settings, no
# globally installed skills, no packages in the system python. All three were
# measured leaking into host runs; the container removes them by construction.
DOCKER_IMAGE = os.environ.get("ARENA_IMAGE", "harden-arena")
DOCKERFILE_DIR = Path(__file__).resolve().parent.parent / "assets" / "arena"
HOME_VOLUME = "harden-arena-home-{h}"
# Per-run clones of the home volume let runs of ONE harness execute
# concurrently: each container gets its own /root, no two CLIs ever share an
# auth file. The name carries the owning PID so a sweep can tell an orphan
# from a live sibling process.
CLONE_VOLUME = "harden-arena-clone-{h}-{pid}-{cond}{n}"
LANE_PARALLEL = int(os.environ.get("ARENA_LANE_PARALLEL", "6"))
# All lanes together stay under this many concurrent containers: agents are
# mostly idle on API calls, but 24 at once is a docker/RAM problem, not a
# measurement.
CONTAINER_SLOTS = threading.Semaphore(int(os.environ.get("ARENA_MAX_CONTAINERS", "8")))
# Harnesses that cannot run in the container at all. Empty today: devin lived
# here until its installer turned out to ship aarch64-linux bundles — the
# "no Linux build" belief lasted exactly until someone checked the install
# script's platform matrix.
DOCKERLESS: tuple[str, ...] = ()
# Only credential files cross into the volume — settings and skills stay behind,
# which is the point. claude on macOS keeps its token in the Keychain, not in a
# file: `--docker-login claude` is the one-time way in.
AUTH_SEEDS: dict[str, tuple[Path, ...]] = {
    "codex": (Path.home() / ".codex" / "auth.json",),
    # grok is deliberately absent: its auth.json is device-bound, and a copy
    # seeded from the host reads as "authenticated" while every run dies with
    # "Not signed in" — measured. `--docker-login grok` is the way in.
    "claude": (Path.home() / ".claude" / ".credentials.json",),
    # config carries org and permissions; credentials.toml carries an API key —
    # `devin auth status` names that path itself when not logged in
    "devin": (Path.home() / ".config" / "devin" / "config.json",
              Path.home() / ".local" / "share" / "devin" / "credentials.toml"),
}
AUTH_PROOF = {
    "codex": ".codex/auth.json",
    "grok": ".grok/auth.json",
    "claude": ".claude/.credentials.json",
}
# Where a file check cannot answer, the CLI itself is the oracle. File
# presence already lied once: a seeded device-bound auth.json read as
# "authenticated" while every run died unauthenticated.
AUTH_PROBE: dict[str, tuple[str, ...]] = {
    "devin": ("devin", "auth", "status"),
}

# Confinement on the HOST, per harness, decided by measurement rather than
# preference — this is the fallback when docker is unavailable or unauthenticated:
#   codex — its own `-s workspace-write`; wrapping it in Seatbelt as well breaks it
#   grok  — its own `--sandbox workspace`; a write to $HOME returns EPERM
#   claude— has none of its own, and runs correctly under Seatbelt
#   devin — its `--sandbox` forces a mode the org policy blocks, and under Seatbelt
#           its ACP session fails to start. Runs unconfined, and the report says so.
# The one that mattered: the host contamination traced to a run was claude's.
SEATBELT = """(version 1)
(allow default)
(deny file-write*)
(allow file-write*
  (subpath (param "WORK"))
  (subpath "/private/tmp") (subpath "/tmp")
  (subpath "/private/var/folders") (subpath "/var/folders")
  (subpath (string-append (param "HOME") "/.claude"))
  (subpath (string-append (param "HOME") "/.cache"))
  (subpath (string-append (param "HOME") "/.config"))
  (subpath (string-append (param "HOME") "/.local")))
"""
# The user's own settings leak into every host run otherwise. Measured on this
# machine: language=Russian (so the agent answered in Russian and a verify
# matching English prose failed everything), effortLevel=xhigh against the
# arena's pinned low, a PreCompact hook, and 34 enabled plugins.
# --settings replaces language, hooks and effort. It does NOT remove plugins:
# their skills stay visible on the host. In docker the question does not arise —
# the container's $HOME never held any of it.
CLEAN_SETTINGS = {"language": "English", "hooks": {}, "enabledPlugins": {}}

SEATBELT_HARNESSES = ("claude",)
UNCONFINED = ("devin",)
# Harnesses whose tool calls the hook records. For the rest, "0 calls" would be
# an absence of instrumentation printed as data — they get "—" and a file-level
# edit count derived from the diff instead.
TRACED = ("claude", "codex")

# Devin rejects the whole permissions block if Exec(sh ...) / Exec(bash ...)
# appear in it — verified: nothing runs with them, the same set works without.
DEVIN_CONFIG = {
    "permissions": {
        "allow": [
            "Read(**)", "Write(**)",
            "Exec(python3)", "Exec(python3 *)", "Exec(python)", "Exec(python *)",
            "Exec(uv)", "Exec(uv *)", "Exec(pip)", "Exec(pip *)",
            "Exec(pytest)", "Exec(pytest *)",
            "Exec(ls)", "Exec(ls *)", "Exec(cat)", "Exec(cat *)",
            "Exec(head)", "Exec(head *)", "Exec(chmod)", "Exec(chmod *)",
            "Exec(mkdir)", "Exec(mkdir *)", "Exec(git)", "Exec(git *)",
        ]
    }
}

TRACE_HOOK = """#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# ///
import json, os, sys
tag = sys.argv[1] if len(sys.argv) > 1 else "?"
try:
    d = json.load(sys.stdin)
except Exception:
    d = {}
i = d.get("tool_input") or {}
arg = i.get("command") or i.get("file_path") or i.get("skill") or ""
with open(os.environ["ARENA_TRACE"], "a", encoding="utf-8") as f:
    f.write(f"{tag}\\t{d.get('tool_name')}\\t{str(arg)[:200]}\\n")
"""

INFRA_MARKERS = [
    r"Could not resolve host", r"unavailable .{0,20}network", r"network is unreachable",
    r"Temporary failure in name resolution",
    r"No space left", r"rate limit", r"429 Too Many",
    r"blocked by the environment",
    # auth failures are infrastructure too: a run that could not start says
    # nothing about the patch
    r"Invalid API key", r"[Nn]ot logged in", r"[Nn]ot signed in",
    r"[Aa]uthentication (failed|required)", r"401 Unauthorized",
    # "command not found" and "permission denied" are deliberately absent:
    # agents probing their sandbox print both into their own transcripts, and
    # a probe is an experimental result. Measured: a patch that told agents to
    # re-run a cited gh command got eight completed runs classified as
    # infrastructure by the old list.
]


# ── target normalisation ───────────────────────────────────────────────────
# Normalise the TARGET, not the command string: by command string a stuck-loop
# detector finds a handful of cases where by target it finds dozens.
def target(cmd: str) -> str:
    c = re.sub(r"^/bin/(zsh|bash)\s+-lc\s+", "", cmd.strip())
    for _ in range(4):
        n = re.sub(r"^\s*cd\s+\S+\s*&&\s*", "", c)
        n = re.sub(r"^\s*source\s+\S+\s*&&\s*", "", n)
        n = re.sub(r"^(env\s+\w+=\S+\s+)+", "", n)
        n = re.sub(r"^\s*\S*/\.venv/bin/(python3?|pytest)\s+(-m\s+)?", "python ", n)
        if n == c:
            break
        c = n
    for raw in c.split("\n"):
        line = raw.strip()
        if line and not re.match(r"^(cd|export|source|set|\.)\s", line):
            c = line
            break
    else:
        c = c.split("\n")[0]
    if c.startswith("/") and " " not in c:
        return f"file:{Path(c).name}"
    if m := re.search(r"pytest\s+(?:[^\s]+\s+)*?([\w/.\-]+\.py(?:::[\w\[\]\-]+)?)", c):
        return f"pytest:{m.group(1)}"
    if m := re.search(r"\b(pre-commit|ruff|mypy|cargo|npm|uv|make|pip|docker)\b\s*(\S*)", c):
        return f"{m.group(1)}:{m.group(2) or '*'}"
    if m := re.match(r"^(\w[\w.\-]*)\s+(?!-)(\S+)", c):
        return f"{m.group(1)}:{m.group(2)[:40]}"
    return (c.split() or ["unknown"])[0][:40]


# ── the host environment, as an agent would see it ─────────────────────────
def host_path() -> str:
    """PATH with uv's ephemeral script environment removed.

    Everything here runs under `uv run`, which puts its own venv first. Handing
    that PATH to an agent would measure the arena's environment rather than the
    machine's — and it silently blinded the contamination guard once already.
    """
    return ":".join(p for p in os.environ.get("PATH", "").split(":")
                    if "/.cache/uv/" not in p and "/uv/environments" not in p)


# ── docker plumbing ────────────────────────────────────────────────────────
def docker_ready() -> bool:
    try:
        r = subprocess.run(["docker", "info", "--format", "ok"],
                           capture_output=True, text=True, timeout=20, check=False)
    except (subprocess.SubprocessError, OSError):
        return False
    return r.returncode == 0


def ensure_image(*, rebuild: bool = False) -> str:
    """Build the arena image if missing; return its id for environment.json."""
    have = subprocess.run(["docker", "image", "inspect", DOCKER_IMAGE,
                           "--format", "{{.Id}}"],
                          capture_output=True, text=True, check=False)
    if have.returncode == 0 and not rebuild:
        return have.stdout.strip()
    print(f"building the {DOCKER_IMAGE} image (a few minutes, once)…")
    b = subprocess.run(["docker", "build", "-t", DOCKER_IMAGE, str(DOCKERFILE_DIR)],
                       capture_output=True, text=True, check=False)
    if b.returncode != 0:
        raise RuntimeError(f"docker build failed:\n{b.stderr[-2000:]}")
    have = subprocess.run(["docker", "image", "inspect", DOCKER_IMAGE,
                           "--format", "{{.Id}}"],
                          capture_output=True, text=True, check=False)
    return have.stdout.strip()


def in_image(inner: list[str], *, volume: str | None = None,
             timeout: int = 60) -> subprocess.CompletedProcess[str]:
    """A throwaway container for probes and seeding — no workdir mounts."""
    cmd = ["docker", "run", "--rm"]
    if volume:
        cmd += ["-v", f"{volume}:/root"]
    return subprocess.run([*cmd, DOCKER_IMAGE, *inner],
                          capture_output=True, text=True, timeout=timeout, check=False)


def seed_auth(harness: str) -> bool:
    """Copy the harness's credential files from the host into its home volume.

    Returns whether the volume ends up authenticated. Nothing else crosses:
    settings, skills and caches stay on the host, which is the point of the
    container.
    """
    vol = HOME_VOLUME.format(h=harness)
    for src in AUTH_SEEDS.get(harness, ()):
        if not src.is_file():
            continue
        rel = src.relative_to(Path.home()).as_posix()
        subprocess.run(
            ["docker", "run", "--rm", "-v", f"{vol}:/root", "-v", f"{src}:/seed:ro",
             DOCKER_IMAGE, "sh", "-c",
             (f"mkdir -p /root/{Path(rel).parent.as_posix()} && "
              f"cp /seed /root/{rel} && chmod 600 /root/{rel}")],
            capture_output=True, text=True, check=False)
    if probe_cmd := AUTH_PROBE.get(harness):
        # rc alone is not the answer: `devin auth status` exits 0 while
        # printing "Not logged in." — the words are the verdict.
        r = in_image(list(probe_cmd), volume=vol)
        return r.returncode == 0 and not re.search(
            r"not (logged|signed) in", r.stdout + r.stderr, re.IGNORECASE)
    proof = AUTH_PROOF.get(harness)
    if not proof:
        return True
    r = in_image(["sh", "-c", f"test -f /root/{proof}"], volume=vol)
    return r.returncode == 0


def docker_wrap(inner: list[str], *, harness: str, work: Path, logdir: Path,
                repo: Path, name: str, envs: dict[str, str],
                home_vol: str | None = None) -> list[str]:
    """Wrap a command to run inside the arena container.

    Mounts: the harness's home volume at /root (a per-run clone when the lane
    runs concurrently), the working copy at /work (rw), the trace/log
    directory at /trace (rw), and the project at /repo read-only — setup.py
    and verify.py live there and may reference files beside themselves, but
    nothing in a run is allowed to write back into the source tree.
    """
    cmd = ["docker", "run", "--rm", "--init", "--name", name,
           "-v", f"{home_vol or HOME_VOLUME.format(h=harness)}:/root",
           "-v", f"{work}:/work", "-v", f"{logdir}:/trace",
           "-v", f"{repo}:/repo:ro", "-w", "/work"]
    for k, v in envs.items():
        cmd += ["-e", f"{k}={v}"]
    return [*cmd, DOCKER_IMAGE, *inner]


def clone_volume(harness: str, cond: str, run: int) -> str:
    """A per-run copy of the harness home volume — the mechanism that lets
    runs of one harness execute concurrently. Lives exactly as long as the
    run (dropped in run_one's finally); sweep_orphan_clones catches what a
    killed process leaves behind."""
    name = CLONE_VOLUME.format(h=harness, pid=os.getpid(), cond=cond, n=run)
    subprocess.run(["docker", "volume", "create", name],
                   capture_output=True, check=True)
    r = subprocess.run(["docker", "run", "--rm",
                        "-v", f"{HOME_VOLUME.format(h=harness)}:/from:ro",
                        "-v", f"{name}:/to", DOCKER_IMAGE,
                        "sh", "-c", "cp -a /from/. /to/"],
                       capture_output=True, text=True, check=False)
    if r.returncode != 0:
        drop_volume(name)
        raise RuntimeError(
            f"volume clone failed for {harness}: {(r.stderr or '').strip()[:200]}")
    return name


def drop_volume(name: str) -> None:
    subprocess.run(["docker", "volume", "rm", "-f", name],
                   capture_output=True, check=False)


def sweep_orphan_clones() -> None:
    """Remove clones whose owning arena process is dead. The PID lives in
    the clone's name; a live sibling process keeps its clones — with per-run
    clones, two arenas on one harness are legal."""
    r = subprocess.run(["docker", "volume", "ls", "-q", "--filter",
                        "name=harden-arena-clone-"],
                       capture_output=True, text=True, check=False)
    for name in (r.stdout or "").split():
        m = re.search(r"harden-arena-clone-[a-z]+-(\d+)-", name)
        if not m:
            continue
        try:
            os.kill(int(m.group(1)), 0)
        except ProcessLookupError:
            drop_volume(name)
        except PermissionError:
            pass


def paths_for(dir_: Path, logdir: Path, tracename: str, *, dockerized: bool) -> dict[str, str]:
    """Where the run's own files appear FROM THE AGENT'S POINT OF VIEW.

    In docker mode the hook path, the trace file and the settings file must be
    container paths — a host-absolute path inside a hook config is a silent
    no-op there, and a hook that never fires reports an agent that never acted.
    """
    if dockerized:
        return {"trace": f"/trace/{tracename}",
                "settings": "/trace/clean-settings.json",
                "hook": "/work/.claude/hooks/trace.py"}
    return {"trace": str(logdir / tracename),
            "settings": str(logdir / "clean-settings.json"),
            "hook": str(dir_ / ".claude" / "hooks" / "trace.py")}


# ── preflight ──────────────────────────────────────────────────────────────
def probe(cmd: list[str]) -> str:
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=60, check=False)
        return (r.stdout or r.stderr).strip().splitlines()[0] if (r.stdout or r.stderr) else ""
    except (subprocess.SubprocessError, OSError):
        return ""


def preflight(out: Path, placement: dict[str, str], image: str) -> Env:
    """Record the environment BEFORE any run — for every place a run happens.

    Without it a control/patch difference can be an environment difference
    rather than a patch effect. With mixed placement (docker plus host
    fallbacks) both environments are recorded, because both are being measured.
    """
    try:
        with urllib.request.urlopen("https://pypi.org/simple/", timeout=8) as r:
            net = str(r.status)
    except (urllib.error.URLError, OSError, ValueError):
        net = "000"

    # NOT sys.executable, and not a bare `python3` either. This script runs under
    # `uv run`, which prepends its own throwaway environment to PATH — so both
    # resolve to an interpreter no agent will ever see. Measured divergence: the
    # arena reported python 3.13.10 with pandas absent, while the interpreter
    # reachable outside uv was 3.13.9 with pandas installed. The contamination
    # guard was passing on a contaminated machine.
    clean = {**os.environ, "PATH": host_path()}

    def importable(mod: str) -> bool:
        return subprocess.run(["python3", "-c", f"import {mod}"],
                              capture_output=True, check=False, env=clean).returncode == 0

    def host_python() -> str:
        r = subprocess.run(["python3", "-V"], capture_output=True, text=True,
                           check=False, env=clean)
        return (r.stdout or r.stderr).strip()

    dockerized = sorted(h for h, p in placement.items() if p == "docker")
    env: Env = {
        "net_pypi": net,
        "system_python": host_python(),
        "system_has_matplotlib": importable("matplotlib"),
        "system_has_pandas": importable("pandas"),
        "uv": probe(["uv", "--version"]),
        "sandbox": {"placement": placement, "image": image or None},
        "models": {"claude": f"{CLAUDE_MODEL}/{EFFORT}", "devin": DEVIN_MODEL,
                   "codex": f"{CODEX_MODEL}/{EFFORT}", "grok": f"{GROK_MODEL}/{EFFORT}"},
        "judge": JUDGE_MODEL,
    }
    if dockerized:
        env["container_python"] = probe(["docker", "run", "--rm", DOCKER_IMAGE,
                                         "python3", "-V"])
        env["container_has_pandas"] = in_image(
            ["python3", "-c", "import pandas"]).returncode == 0
        env["container_has_matplotlib"] = in_image(
            ["python3", "-c", "import matplotlib"]).returncode == 0
    for h, where in placement.items():
        version_cmd = {"devin": [h, "version"]}.get(h, [h, "--version"])
        env[h] = (probe(["docker", "run", "--rm",
                         "-v", f"{HOME_VOLUME.format(h=h)}:/root",
                         DOCKER_IMAGE, *version_cmd])
                  if where == "docker" else probe(version_cmd))
    (out / "environment.json").write_text(json.dumps(env, indent=2), encoding="utf-8")

    print("-- environment --")
    print("  placement: " + ", ".join(f"{h}={p}" for h, p in sorted(placement.items())))
    print(f"  network to pypi: {env['net_pypi']}  | host python: {env['system_python']}"
          + (f"  | container python: {env.get('container_python')}" if dockerized else ""))
    print(f"  already in host python: matplotlib={env['system_has_matplotlib']} "
          f"pandas={env['system_has_pandas']}")
    if env["net_pypi"] != "200":
        print("  WARNING: no network - dependency installation will fail, runs are not comparable")
    hosted = sorted(h for h, p in placement.items() if p != "docker")
    if hosted and (env["system_has_matplotlib"] or env["system_has_pandas"]):
        print(f"  WARNING: packages already in the host python and {', '.join(hosted)} "
              "run(s) on the host - those runs compete on unequal terms")
    print()
    return env


# ── one working copy ───────────────────────────────────────────────────────
def prepare(fixture: Path, dir_: Path, hook_path: str, condition: str) -> None:
    if dir_.exists():
        shutil.rmtree(dir_)
    # verify.py stays out for the same reason setup.py does: an agent that
    # reads the examiner learns what is measured — observed live, runs
    # grepping verify for its own pass conditions.
    shutil.copytree(fixture, dir_,
                    ignore=shutil.ignore_patterns("control.md", "patch.md", "task.md",
                                                  "README.md", "setup.py", "verify.py"))
    # control.md / patch.md become AGENTS.md here and nowhere else
    source = fixture / ("patch.md" if condition == "patch" else "control.md")
    shutil.copy(source, dir_ / "AGENTS.md")
    # deliberately no CLAUDE.md: AGENTS.md is what is under test

    hooks = dir_ / ".claude" / "hooks"
    hooks.mkdir(parents=True, exist_ok=True)
    hook = hooks / "trace.py"
    hook.write_text(TRACE_HOOK, encoding="utf-8")
    hook.chmod(0o755)

    def entry(tag: str) -> dict[str, object]:
        return {"matcher": "", "hooks": [{"type": "command", "command": "uv",
                "args": ["run", hook_path, tag]}]}

    (dir_ / ".claude" / "settings.json").write_text(json.dumps(
        {"hooks": {"PreToolUse": [entry("CALL")], "PostToolUseFailure": [entry("FAIL")]}},
        indent=2), encoding="utf-8")

    (dir_ / ".devin").mkdir(exist_ok=True)
    (dir_ / ".devin" / "config.json").write_text(
        json.dumps(DEVIN_CONFIG, indent=2), encoding="utf-8")

    (dir_ / ".codex").mkdir(exist_ok=True)
    codex_cmd = f"uv run {hook_path} "
    (dir_ / ".codex" / "hooks.json").write_text(json.dumps({"hooks": {
        "PreToolUse": [{"matcher": "",
                        "hooks": [{"type": "command", "command": codex_cmd + "CALL"}]}],
        "PostToolUse": [{"matcher": "",
                         "hooks": [{"type": "command", "command": codex_cmd + "POST"}]}],
    }}, indent=2), encoding="utf-8")


# ── snapshot and diff: what did the agent actually change ──────────────────
def _texty(b: bytes) -> str | None:
    if len(b) > SNAP_LIMIT:
        return None
    try:
        return b.decode("utf-8")
    except UnicodeDecodeError:
        return None


def snap_tree(root: Path) -> dict[str, str]:
    """Content of every file; hash stands in for binary or oversized ones.

    `_*` files are the arena's own artefacts and `.git/` internals churn on
    every git command — both would put noise in the diff the judge reads.
    """
    snap: dict[str, str] = {}
    for f in sorted(root.rglob("*")):
        if not f.is_file() or f.is_symlink():
            continue
        rel = f.relative_to(root).as_posix()
        if rel.startswith(".git/") or f.name.startswith("_"):
            continue
        b = f.read_bytes()
        text = _texty(b)
        snap[rel] = text if text is not None else \
            f"<binary or large: {len(b)} bytes, sha1 {hashlib.sha1(b).hexdigest()[:12]}>"  # noqa: S324
    return snap


def tree_diff(before: dict[str, str], root: Path) -> tuple[str, list[str]]:
    """Unified diff of the working copy against its pre-run snapshot."""
    after = snap_tree(root)
    chunks: list[str] = []
    changed: list[str] = []
    for rel in sorted(set(before) | set(after)):
        a, b = before.get(rel), after.get(rel)
        if a == b:
            continue
        changed.append(rel)
        chunks.extend(difflib.unified_diff(
            (a or "").splitlines(), (b or "").splitlines(),
            fromfile=f"a/{rel}" if a is not None else "/dev/null",
            tofile=f"b/{rel}" if b is not None else "/dev/null", lineterm=""))
    return "\n".join(chunks), changed


# ── harness output: text, turns and tokens ─────────────────────────────────
def scan_usage(obj: Any) -> dict[str, int]:
    """Harvest token counters from a harness's JSON, whatever its shape.

    Streams report cumulative totals per event, so the maximum per counter is
    the final figure. Key names differ per vendor; the mapping is the contract.
    """
    keys = {"input_tokens": "in", "prompt_tokens": "in",
            "output_tokens": "out", "completion_tokens": "out",
            "cached_input_tokens": "cached", "cache_read_input_tokens": "cached",
            "total_tokens": "total", "total_token_usage": "total"}
    found: dict[str, int] = {}

    def walk(o: Any) -> None:
        if isinstance(o, dict):
            for k, v in o.items():
                if k in keys and isinstance(v, (int, float)) and v >= 0:
                    tag = keys[k]
                    found[tag] = max(found.get(tag, 0), int(v))
                else:
                    walk(v)
        elif isinstance(o, list):
            for x in o:
                walk(x)

    walk(obj)
    return found


def extract_text(obj: Any) -> str:
    """The model's final prose out of an arbitrary result envelope."""
    for key in ("result", "response", "text", "content", "message", "output"):
        v = obj.get(key) if isinstance(obj, dict) else None
        if isinstance(v, str) and v.strip():
            return v
        if isinstance(v, (dict, list)):
            inner = extract_text(v)
            if inner:
                return inner
    if isinstance(obj, list):
        parts = [extract_text(x) for x in obj]
        return "\n".join(p for p in parts if p)
    return ""


def parse_claude(stdout: str) -> tuple[str, dict[str, Any]]:
    """`claude -p --output-format json` → (final text, usage fields)."""
    try:
        d = json.loads(stdout)
    except ValueError:
        return stdout, {}
    u = scan_usage(d)
    fields: dict[str, Any] = {"turns": d.get("num_turns"),
                              "tok_in": u.get("in"), "tok_out": u.get("out"),
                              "cost_usd": d.get("total_cost_usd")}
    return extract_text(d) or stdout, fields


def parse_codex(stdout: str) -> tuple[str, dict[str, Any]]:
    """`codex exec --json` JSONL → (agent messages, usage fields).

    Hook chatter and log lines interleave with the JSON stream, so every line
    is parsed independently and non-JSON lines are skipped. Falls back to the
    plain-mode trailer ("tokens used\\n44 053") when no JSON events are found.
    """
    texts: list[str] = []
    turns = 0
    usage: dict[str, int] = {}
    seen = False
    for raw in stdout.splitlines():
        line = raw.strip()
        if not line.startswith("{"):
            continue
        try:
            e = json.loads(line)
        except ValueError:
            continue
        seen = True
        item = e.get("item") or {}
        if item.get("type") == "agent_message" and item.get("text"):
            texts.append(str(item["text"]))
        if str(e.get("type", "")).startswith("turn"):
            turns += 1
        for k, v in scan_usage(e).items():
            usage[k] = max(usage.get(k, 0), v)
    if seen and (texts or usage):
        fields: dict[str, Any] = {"turns": turns or None,
                                  "tok_in": usage.get("in"), "tok_out": usage.get("out")}
        return "\n\n".join(texts) or stdout, fields
    m = re.search(r"tokens used\s*\n?\s*([\d][\d\s., ]*)", stdout)
    total = int(re.sub(r"\D", "", m.group(1))) if m else None
    return stdout, {"tok_total": total} if total else {}


def parse_grok(stdout: str) -> tuple[str, dict[str, Any]]:
    """`grok -p --output-format json` → (final text, usage fields)."""
    try:
        d = json.loads(stdout)
    except ValueError:
        return stdout, {}
    u = scan_usage(d)
    fields: dict[str, Any] = {"tok_in": u.get("in"), "tok_out": u.get("out")}
    if u.get("total") and not (u.get("in") or u.get("out")):
        fields = {"tok_total": u["total"]}
    return extract_text(d) or stdout, fields


PARSERS = {"claude": parse_claude, "codex": parse_codex, "grok": parse_grok}


def normalize_output(harness: str, stdout: str, stderr: str) -> tuple[str, dict[str, Any]]:
    """One shape for every harness: readable prose in _agent-output.txt, usage
    fields for the row. Fixtures grep the output file, so it must stay prose
    whatever the CLI's native format is."""
    parser = PARSERS.get(harness)
    text, fields = parser(stdout) if parser else (stdout, {})
    if stderr.strip():
        text = f"{text}\n\n--- stderr ---\n{stderr}"
    return text, fields


def row_tokens(row: Row) -> int | None:
    if row.get("tok_total") is not None:
        return int(row["tok_total"])
    if row.get("tok_in") is None and row.get("tok_out") is None:
        return None
    return int(row.get("tok_in") or 0) + int(row.get("tok_out") or 0)


def declassify_infra(row: Row) -> Row:
    """Clear the infra flag when the vendor envelope parsed.

    Turns or token usage in the row mean the harness ran its protocol to the
    end — whatever infra-looking text sits in the transcript came from the
    agent's own commands, not from a run that failed to start. Marker-based
    classification keeps its say only where no envelope exists to overrule it
    (devin has none; a launch or auth failure parses nothing).
    """
    if row.get("infra_failure") and (
            row.get("turns") is not None or row_tokens(row) is not None):
        row["infra_failure"] = False
    return row


# ── the judge ──────────────────────────────────────────────────────────────
def scrub(text: str) -> str:
    """Strip model self-identification before the judge sees a transcript.

    The judge is one of the contestants' siblings; a transcript that announces
    its brand invites brand bias. Guarded on both sides so paths (`.claude/`)
    and run-directory names (`codex-1`) survive — only prose mentions go.

    Also strips NUL bytes: judge evidence travels as a subprocess argument,
    and one agent-written file with an embedded NUL killed the whole judge
    phase after 24 clean runs (Popen refuses argv containing NUL).
    """
    text = text.replace("\x00", "")
    return re.sub(r"(?i)(?<![\w./-])(claude|codex|grok|devin|gpt[\w.-]*|sonnet|opus"
                  r"|haiku|anthropic|openai|xai|cognition)(?![\w-])", "the-agent", text)


def tree_listing(dir_: Path) -> str:
    """The working copy as it stands after the run — the judge's fact-check.

    A transcript is the agent's story; the tree is what is actually there. The
    first judged fabrication ("the `run` skill already exists in this project",
    scored 10/10) was invisible in the story and one line in the tree.
    """
    files = sorted(f.relative_to(dir_).as_posix() for f in dir_.rglob("*")
                   if f.is_file() and not f.name.startswith("_")
                   and not f.relative_to(dir_).as_posix().startswith(".git/"))
    shown = files[:TREE_FOR_JUDGE]
    more = len(files) - len(shown)
    return "\n".join(shown) + (f"\n… and {more} more files" if more > 0 else "")


def trace_summary(dir_: Path) -> str:
    """What the agent actually did, one line per tool call — or an honest
    statement that this harness records no trace."""
    trace = dir_ / "_trace.tsv"
    if not trace.is_file() or not trace.read_text(encoding="utf-8").strip():
        return "(no tool-call trace is recorded for this harness)"
    lines = []
    for raw in trace.read_text(encoding="utf-8", errors="replace").splitlines():
        parts = raw.split("\t")
        if len(parts) >= MIN_TRACE_FIELDS:
            arg = parts[2][:80] if len(parts) > MIN_TRACE_FIELDS else ""
            lines.append(f"{parts[0]} {parts[1]}: {arg}".rstrip())
    shown = lines[:TRACE_FOR_JUDGE]
    more = len(lines) - len(shown)
    return "\n".join(shown) + (f"\n… and {more} more calls" if more > 0 else "")


def changed_paths(diff: str) -> list[str]:
    """The files a unified diff touched, from its `+++ b/` headers."""
    out: list[str] = []
    for line in diff.splitlines():
        if line.startswith("+++ b/"):
            p = line[len("+++ b/"):].strip()
            if p and p != "/dev/null" and p not in out:
                out.append(p)
    return out[:CHANGED_FOR_JUDGE]


def judge_prompt(task: str, diff: str, tail: str, tree: str, trace: str,  # noqa: PLR0917
                 changed: str) -> str:
    return (
        "You are grading one automated coding-agent run, blind: you do not know "
        "which model produced it, and any self-identification left in the "
        "transcript must be ignored. Grade only the evidence below.\n\n"
        "== the task the agent was given ==\n" + task.strip() +
        "\n\n== files on disk after the run ==\n" + tree.strip() +
        "\n\n== every tool call the agent made ==\n" + trace.strip() +
        "\n\n== files the agent changed (unified diff; empty means it changed nothing) ==\n"
        + (diff.strip() or "(no changes)") +
        "\n\n== FULL current content of the changed files ==\n"
        + (changed.strip() or "(no files changed)") +
        "\n\n== end of the agent's transcript ==\n" + tail.strip() +
        "\n\nScore the run:\n"
        "- correctness and completeness of the result against the task: 0-6\n"
        "- minimality: a clean, small change that solves it: 0-2\n"
        "- discipline: no collateral edits, no ignored constraints: 0-2\n\n"
        "The transcript is the agent's story; the tree and the calls are the "
        "facts. Check every claim about the PROJECT against them — a project "
        "file or directory said to exist that is not listed, or a check said "
        "to have run with no matching call, is a fabrication: list it in "
        "unsupported_claims and score discipline 0. The changed files' FULL "
        'content is in evidence above — a claim about one of them ("no other '
        'issues in this file") is checkable and must be checked. Boundaries '
        "of this evidence, where you must neither confirm nor punish: the "
        "agent's harness ships built-in skills and tools not listed here; for "
        "files the agent did NOT change, the tree gives paths only; the calls "
        "are listed WITHOUT their outputs, so what a command printed is not "
        "in evidence; and the outside world — catalogues, packages, web "
        "results — is not in evidence at all. Mention such claims in the "
        "note if the reasoning leans on them.\n\n"
        "Do not call any tools — everything you may use is above. "
        'Answer with ONLY this JSON object, nothing else:\n'
        '{"solved": true, "score": 0, "collateral": [], '
        '"unsupported_claims": [], "note": "<=25 words"}')


def parse_judgement(raw: str) -> dict[str, Any] | None:
    """The JSON object out of the judge's reply, fences and prose tolerated."""
    m = re.search(r"\{.*\}", raw, flags=re.DOTALL)
    if not m:
        return None
    try:
        d = json.loads(m.group(0))
    except ValueError:
        return None
    if not isinstance(d, dict) or "score" not in d:
        return None
    try:
        score = max(0, min(JUDGE_MAX_SCORE, int(d["score"])))
    except (TypeError, ValueError):
        return None
    return {"solved": bool(d.get("solved")), "score": score,
            "collateral": [str(x)[:120] for x in (d.get("collateral") or [])][:5],
            "unsupported_claims": [str(x)[:160]
                                   for x in (d.get("unsupported_claims") or [])][:5],
            "note": str(d.get("note") or "")[:200]}


def judge_one(row: Row, task: str, logdir: Path) -> dict[str, Any] | None:
    """One blind judgement, on the host, through the claude CLI.

    The judge never sees the harness name, the condition, or verify's verdict —
    it grades the diff and the transcript tail, and disagreement with verify is
    reported rather than resolved.
    """
    dir_ = Path(row["dir"])
    diff_file = dir_ / "_diff.patch"
    diff_text = diff_file.read_text(encoding="utf-8", errors="replace") \
        if diff_file.exists() else ""
    out_file = dir_ / "_agent-output.txt"
    tail = out_file.read_text(encoding="utf-8", errors="replace")[-TAIL_FOR_JUDGE:] \
        if out_file.exists() else ""
    # The changed files, in full: the run directory is mounted host-side, so
    # the evidence is already there — this shows it. A claim like "no other
    # issues in this file" was taken on faith once; about a file the agent
    # itself touched, it is checkable and now checked.
    blobs = []
    for rel in changed_paths(diff_text):
        f = dir_ / rel
        if f.is_file():
            body = f.read_text(encoding="utf-8", errors="replace")[:CONTENT_FOR_JUDGE]
            blobs.append(f"--- {rel} ---\n{body}")
    prompt = judge_prompt(task, scrub(diff_text[:DIFF_FOR_JUDGE]), scrub(tail),
                          tree_listing(dir_) if dir_.is_dir() else "(gone)",
                          scrub(trace_summary(dir_)),
                          scrub("\n\n".join(blobs)))
    settings = logdir / "clean-settings.json"
    # --max-turns 4, not 1: with every tool disallowed the model sometimes tries
    # one anyway, the denial consumes a turn, and at 1 the reply comes back
    # empty with rc 1 — measured on the first judged run. The spare turns are
    # the room to fail a tool call and still answer in text.
    cmd = ["claude", "-p", prompt, "--model", JUDGE_MODEL, "--settings", str(settings),
           "--max-turns", "4", "--output-format", "json",
           "--disallowedTools", "Bash,Edit,Write,Read,Glob,Grep,WebFetch,WebSearch,Task,Skill"]
    for _ in range(2):
        try:
            r = subprocess.run(cmd, capture_output=True, text=True,
                               timeout=JUDGE_TIMEOUT_S, check=False,
                               env={**os.environ, "PATH": host_path()})
        except (subprocess.SubprocessError, OSError):
            continue
        try:
            envelope = json.loads(r.stdout)
            reply = extract_text(envelope) or r.stdout
        except ValueError:
            reply = r.stdout
        if verdict := parse_judgement(reply):
            return verdict
    return None


def representatives(rows: list[Row]) -> list[Row]:
    """One run per (harness, condition): the first whose verify verdict matches
    the cell's majority outcome. Judging all N runs of a cell mostly re-scores
    the same behaviour — at ~60-90s per call, that is expense, not information.
    """
    by: defaultdict[tuple[str, str], list[Row]] = defaultdict(list)
    for r in rows:
        by[(r["harness"], r["condition"])].append(r)
    picked = []
    for cell in by.values():
        passes = sum(1 for r in cell if r["verdict"] == "pass")
        majority = "pass" if passes * 2 >= len(cell) else "fail"
        picked.append(next((r for r in cell if r["verdict"] == majority), cell[0]))
    return picked


def judge_all(rows: list[Row], task: str, logdir: Path, *,
              everything: bool = False) -> None:
    subset = rows if everything else representatives(rows)
    print(f"-- judge ({JUDGE_MODEL}, blind, {len(subset)} of {len(rows)} runs, "
          f"{JUDGE_PARALLEL} at a time) --")
    with ThreadPoolExecutor(max_workers=JUDGE_PARALLEL) as pool:
        futures = {pool.submit(judge_one, row, task, logdir): row for row in subset}
        for fut in as_completed(futures):
            row = futures[fut]
            # The judge is advisory: one failed grading degrades to an
            # ungraded run, never to a dead report — a NUL byte in one
            # run's evidence once killed the whole phase after 24 clean runs.
            try:
                verdict = fut.result()
            except Exception as e:  # noqa: BLE001
                verdict = None
                print(f"  judge failed for {row['harness']}/{row['condition']}"
                      f"#{row['run']}: {type(e).__name__}: {e}")
            row["judge"] = verdict
            mark = f"{verdict['score']}/10" if verdict else "no verdict"
            print(f"  {row['harness']:<7} {row['condition']:<7} #{row['run']}  {mark}")
    print()


# ── run one combination ────────────────────────────────────────────────────
def run_setup(fixture: Path, dir_: Path, condition: str,
              docker: dict[str, Any] | None) -> None:
    """Optional per-run setup, for state a copied tree cannot carry: a git repo,
    a venv, a seeded database. Runs with the working copy as cwd, and is NOT
    copied in — an agent that reads the setup script learns what the fixture is
    testing, which is the one thing it must not know.

    ARENA_CONDITION so setup can seed one side only. Needed for the case where
    the artefact under test IS an installed skill: a skill present in both
    copies is discovered by description in both, and the control stops being a
    baseline.
    """
    setup = fixture / "setup.py"
    if not setup.is_file():
        return
    if docker:
        inner = ["uv", "run", f"/repo/{docker['fixture_rel']}/setup.py"]
        cmd = docker_wrap(inner, harness=docker["harness"], work=dir_,
                          logdir=docker["logdir"], repo=docker["repo"],
                          name=f"{docker['name']}-setup",
                          envs={"ARENA_CONDITION": condition},
                          home_vol=docker.get("home_vol"))
        env = dict(os.environ)
    else:
        cmd = ["uv", "run", str(setup)]
        env = {**os.environ, "ARENA_CONDITION": condition, "PATH": host_path()}
    r = subprocess.run(cmd, check=False, cwd=dir_, capture_output=True, text=True,
                       timeout=SETUP_TIMEOUT_S, env=env)
    if r.returncode != 0:
        # The FULL stderr, not a 400-char slice: a truncated traceback once cut
        # off exactly at the exception type, and the failure was transient — the
        # one situation where the evidence cannot be re-collected on demand.
        msg = f"fixture setup.py failed in {dir_}: {(r.stderr or r.stdout).strip()[:4000]}"
        raise RuntimeError(msg)


def command_for(harness: str, task: str, settings: str, *,
                dockerized: bool = False) -> list[str]:
    """The harness invocation — and in docker, WITHOUT its own sandbox.

    The container is the boundary there; a second, inner sandbox measures the
    friction of two nested confinements, not the patch. Observed on the first
    containerised codex run: "blocked by the execution sandbox before the
    command ran", retries, wasted turns.
    """
    if harness == "claude":
        return ["claude", "-p", task, "--model", CLAUDE_MODEL, "--effort", EFFORT,
                "--allowedTools", "Read,Edit,Write,Bash,Glob,Grep,Skill,TodoWrite",
                "--settings", settings, "--output-format", "json"]
    if harness == "devin":
        # --permission-mode dangerous is deliberate. The Exec allowlist gives FALSE
        # control: Devin only checks the first binary of a chain, so one permitted
        # `cd` opens anything after `&&`. An explicit flag beats an illusion of
        # constraint; safety rests on the run directory being throwaway.
        return ["devin", "--model", DEVIN_MODEL, "--respect-workspace-trust", "false",
                "--permission-mode", "dangerous", "-p", task]
    if harness == "grok":
        # --always-approve for the same reason the others get their flags: the run
        # directory is throwaway, and a prompt for permission in a headless run is
        # a hang, not a safeguard.
        cmd = ["grok", "-p", task, "--model", GROK_MODEL,
               "--reasoning-effort", EFFORT, "--always-approve",
               "--output-format", "json"]
        if not dockerized:
            cmd += ["--sandbox", "workspace"]
        return cmd
    # bypass-hook-trust: hooks are arena-generated, directory is throwaway.
    # network_access: workspace-write disables the network, and without it the
    # arena measures connectivity instead of the patch.
    sandbox = (["-s", "danger-full-access"] if dockerized else
               ["-s", "workspace-write",
                "-c", "sandbox_workspace_write.network_access=true"])
    return ["codex", "exec", "--skip-git-repo-check", *sandbox,
            "--dangerously-bypass-hook-trust", "--model", CODEX_MODEL,
            "-c", f"model_reasoning_effort={EFFORT}", "--json", task]


def seatbelt_prefix(work: Path, profile: Path) -> list[str]:
    """Wrap a command in macOS Seatbelt. Empty list where that is unavailable."""
    if sys.platform != "darwin" or not Path("/usr/bin/sandbox-exec").exists():
        return []
    profile.write_text(SEATBELT, encoding="utf-8")
    return ["/usr/bin/sandbox-exec", "-f", str(profile),
            "-D", f"WORK={work}", "-D", f"HOME={Path.home()}"]


def collect(dir_: Path, harness: str, condition: str, run: int,  # noqa: PLR0917
            duration: int, verdict_code: int, trace: Path) -> Row:
    traced = harness in TRACED
    calls = fails = edits = 0
    by_target: Counter[str] = Counter()
    if trace.exists():
        for line in trace.read_text(encoding="utf-8", errors="replace").splitlines():
            parts = line.split("\t")
            if len(parts) < MIN_TRACE_FIELDS:
                continue
            tag, tool, arg = parts[0], parts[1], (parts[2] if len(parts) > MIN_TRACE_FIELDS else "")
            if tag == "CALL":
                calls += 1
                if tool in ("Write", "Edit", "NotebookEdit", "write", "edit", "apply_patch"):
                    edits += 1
            elif tag == "FAIL":
                fails += 1
                by_target[target(arg or tool)] += 1

    out_file = dir_ / "_agent-output.txt"
    out_text = out_file.read_text(encoding="utf-8", errors="replace") if out_file.exists() else ""
    if fails == 0 and harness in ("codex", "devin", "grok"):
        fails = len(re.findall(
            r"Exit code:?\s*[1-9]\d*|exited 1|command not found|Traceback", out_text))

    stuck = {t: n for t, n in by_target.items() if n >= STUCK_THRESHOLD}
    hits = sorted({m.group(0)[:60] for p in INFRA_MARKERS for m in re.finditer(p, out_text)})

    return {
        "harness": harness, "condition": condition, "run": run,
        "verdict": "pass" if verdict_code == 0 else "fail",
        "duration_sec": duration,
        # None, not 0, where there is no instrumentation: a zero that means
        # "nothing was measured" reads as "the agent did nothing", and did.
        "tool_calls": calls if traced else None,
        "tool_failures": fails,
        "fail_per_10_calls": round(10 * fails / calls, 1) if traced and calls else None,
        "edits": edits if traced else None,
        "stuck_targets": stuck, "stuck_total": sum(stuck.values()),
        # an infrastructure failure is not an experimental result: it says nothing
        # about the patch and must be excluded from the verdict, not counted as fail
        "infra_failure": bool(hits) and verdict_code != 0,
        "infra_hits": hits[:5], "output_chars": len(out_text), "dir": str(dir_),
    }


def run_one(fixture: Path, out: Path, harness: str, condition: str,  # noqa: PLR0917
            run: int, task: str, logdir: Path, docker_ctx: dict[str, Any] | None) -> Row:
    dir_ = out / condition / f"{harness}-{run}"
    tracename = f"{condition}-{harness}-{run}.trace"
    trace = logdir / tracename
    trace.write_text("", encoding="utf-8")
    p = paths_for(dir_, logdir, tracename, dockerized=docker_ctx is not None)
    prepare(fixture, dir_, p["hook"], condition)

    name = f"arena-{condition}-{harness}-{run}-{os.getpid()}"
    docker = ({**docker_ctx, "harness": harness, "name": name, "logdir": logdir}
              if docker_ctx is not None else None)
    run_setup(fixture, dir_, condition, docker)
    snapshot = snap_tree(dir_)

    t0 = time.time()
    stdout = stderr = ""
    try:
        cmd = command_for(harness, task, p["settings"], dockerized=docker is not None)
        if docker:
            cmd = docker_wrap(cmd, harness=harness, work=dir_, logdir=logdir,
                              repo=docker["repo"], name=name,
                              envs={"ARENA_TRACE": p["trace"]},
                              home_vol=docker.get("home_vol"))
            env = dict(os.environ)
        else:
            if harness in SEATBELT_HARNESSES:
                # Profile lives beside the trace logs, outside the run directory: a
                # file appearing mid-session inside the working copy gets read as
                # injection, which is how the trace hook learned to live elsewhere.
                cmd = seatbelt_prefix(dir_, logdir / f"{harness}-{condition}-{run}.sb") + cmd
            # host_path: the agent must see the machine, not uv's script venv
            env = {**os.environ, "ARENA_TRACE": p["trace"], "PATH": host_path()}
        r = subprocess.run(cmd, cwd=dir_, env=env,
                           capture_output=True, text=True, stdin=subprocess.DEVNULL,
                           timeout=HARNESS_TIMEOUT_S, check=False)
        stdout, stderr = r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        stderr = f"arena: harness timed out after {HARNESS_TIMEOUT_S}s"
        if docker:
            # the client timed out, the container did not: kill it or it runs on
            subprocess.run(["docker", "kill", name], capture_output=True, check=False)
    except OSError as e:
        stderr = f"arena: could not launch {harness}: {e}"
    duration = int(time.time() - t0)

    text, usage = normalize_output(harness, stdout, stderr)
    (dir_ / "_agent-output.txt").write_text(text, encoding="utf-8")
    if text != stdout and stdout:
        (dir_ / "_agent-raw.txt").write_text(stdout, encoding="utf-8")

    diff, changed = tree_diff(snapshot, dir_)
    (dir_ / "_diff.patch").write_text(diff[:SNAP_LIMIT], encoding="utf-8")

    # `uv run`, not sys.executable: verify.py is a PEP 723 script and may declare
    # dependencies. Running it with the arena's own interpreter ignores that
    # header, and every run then fails on ModuleNotFoundError — which reads as
    # "the task was not solved" for every harness at once. In docker mode verify
    # runs INSIDE the container: it checks the same environment the agent had.
    if docker:
        vcmd = docker_wrap(["uv", "run", f"/repo/{docker['fixture_rel']}/verify.py"],
                           harness=harness, work=dir_, logdir=logdir,
                           repo=docker["repo"], name=f"{name}-verify", envs={},
                           home_vol=docker.get("home_vol"))
        venv_ = dict(os.environ)
    else:
        vcmd = ["uv", "run", str(fixture / "verify.py")]
        venv_ = {**os.environ, "PATH": host_path()}
    v = subprocess.run(vcmd, cwd=dir_, capture_output=True, text=True,
                       check=False, env=venv_)
    (dir_ / "_verify.txt").write_text(v.stdout + v.stderr, encoding="utf-8")
    if trace.exists():
        shutil.copy(trace, dir_ / "_trace.tsv")

    row = collect(dir_, harness, condition, run, duration, v.returncode, trace)
    row.update(usage)
    declassify_infra(row)
    row.setdefault("turns", None)
    row["sandbox"] = "docker" if docker else (
        "seatbelt" if harness in SEATBELT_HARNESSES else
        "none" if harness in UNCONFINED else "own")
    if row["edits"] is None:
        # untraced harnesses: the diff still shows what changed, file-level
        row["edits"] = len([c for c in changed if not c.startswith(".")]) or None
    row["changed_files"] = len(changed)
    tok = row_tokens(row)
    print(f"  {harness:<7} {condition:<7} #{run}  "
          f"{'PASS' if v.returncode == 0 else 'FAIL'}  {duration:>3}s"
          f"  [{row['sandbox']}]" + (f"  {tok:,} tok" if tok else ""))
    return row


def run_harness_lane(fixture: Path, out: Path, harness: str, runs: int,  # noqa: PLR0917
                     task: str, logdir: Path, docker_ctx: dict[str, Any] | None,
                     stop: threading.Event) -> tuple[list[Row], RuntimeError | None]:
    """Every run of ONE harness. Lanes run concurrently, one per harness —
    and in docker mode the runs INSIDE a lane run concurrently too: each gets
    its own clone of the home volume, so no two CLIs ever share an auth file,
    and the seed volume itself never takes part in a run. Host mode keeps the
    original order — there is one real home and no way to clone it.

    A broken setup.py breaks every run identically, so the first failure
    raises the stop flag: sequential mode exits between runs, parallel mode
    skips whatever has not started yet.
    """
    if docker_ctx is None:
        rows: list[Row] = []
        for i in range(1, runs + 1):
            for cond in ("control", "patch"):
                if stop.is_set():
                    return rows, None
                try:
                    rows.append(run_one(fixture, out, harness, cond, i, task,
                                        logdir, docker_ctx))
                except RuntimeError as e:
                    stop.set()
                    return rows, e
        return rows, None

    def one(i: int, cond: str) -> Row:
        if stop.is_set():
            raise RuntimeError("lane stopped before this run started")
        ctx = {**docker_ctx}
        with CONTAINER_SLOTS:
            clone = clone_volume(harness, cond, i)
            try:
                ctx["home_vol"] = clone
                return run_one(fixture, out, harness, cond, i, task, logdir, ctx)
            finally:
                drop_volume(clone)

    combos = [(i, cond) for i in range(1, runs + 1) for cond in ("control", "patch")]
    rows = []
    err: RuntimeError | None = None
    with ThreadPoolExecutor(max_workers=min(len(combos), LANE_PARALLEL)) as pool:
        futures = {pool.submit(one, i, cond): (i, cond) for i, cond in combos}
        for fut in as_completed(futures):
            try:
                rows.append(fut.result())
            except RuntimeError as e:
                stop.set()
                err = err or e
    rows.sort(key=lambda r: (r["condition"], r["run"]))
    return rows, err


# ── report ─────────────────────────────────────────────────────────────────
def _median(rows: list[Row], key: str) -> float | None:
    vals = [r[key] for r in rows if r.get(key) is not None]
    return statistics.median(vals) if vals else None


def _mean_judge(rows: list[Row]) -> float | None:
    vals = [r["judge"]["score"] for r in rows if r.get("judge")]
    return round(sum(vals) / len(vals), 1) if vals else None


def aggregate(rows: list[Row]) -> dict[tuple[str, str], Stats]:
    by: defaultdict[tuple[str, str], list[Row]] = defaultdict(list)
    for r in rows:
        by[(r["harness"], r["condition"])].append(r)
    out: dict[tuple[str, str], Stats] = {}
    for k, v in by.items():
        clean = [r for r in v if not r.get("infra_failure")]
        tokens = [t for r in clean if (t := row_tokens(r)) is not None]
        out[k] = {"n": len(v), "n_clean": len(clean), "infra": len(v) - len(clean),
                  "pass": sum(1 for r in clean if r["verdict"] == "pass"),
                  "fails": _median(clean, "tool_failures"),
                  "rate": _median(clean, "fail_per_10_calls"),
                  "calls": _median(clean, "tool_calls"), "edits": _median(clean, "edits"),
                  "stuck": _median(clean, "stuck_total"), "sec": _median(clean, "duration_sec"),
                  "turns": _median(clean, "turns"),
                  "tokens": statistics.median(tokens) if tokens else None,
                  "judge": _mean_judge(clean)}
    return out


def _num(x: float | None) -> str:
    return "—" if x is None else f"{x:.0f}"


def _tok(x: float | None) -> str:
    if x is None:
        return "—"
    return f"{x / 1000:.1f}k" if x >= 1000 else f"{x:.0f}"  # noqa: PLR2004


def _judge_cell(row: Row) -> str:
    j = row.get("judge")
    return f"{j['score']}/10" if j else "—"


def report(rows: list[Row], env: Env, runs: int, out_file: Path) -> str:
    agg = aggregate(rows)
    harnesses = sorted({h for h, _ in agg})
    infra_total = sum(1 for r in rows if r.get("infra_failure"))
    lines = ["# Arena result\n",
         f"Runs per combination: **{runs}**. Total runs: **{len(rows)}**."
         + (f" Of those, infrastructure failures: **{infra_total}** (excluded from the verdict)."
            if infra_total else "")]

    placement: dict[str, str] = (env.get("sandbox") or {}).get("placement", {})
    hosted = sorted(h for h in harnesses if placement.get(h) != "docker")
    unconfined = sorted({r["harness"] for r in rows
                         if r["harness"] in UNCONFINED and r.get("sandbox") == "none"})
    lines += ["\n## Environment\n",
              ("- sandbox: " + ", ".join(f"{h}=`{placement.get(h, '?')}`"
                                         for h in harnesses)),
              (f"- network to pypi: `{env.get('net_pypi')}` · "
               f"host python: `{env.get('system_python')}`"
               + (f" · container python: `{env.get('container_python')}`"
                  if env.get("container_python") else "")),
          (f"- already in host python: matplotlib=`{env.get('system_has_matplotlib')}` "
           f"pandas=`{env.get('system_has_pandas')}`"),
          ("- models: " + " ".join(f"{k}=`{v}`"
                                   for k, v in (env.get("models") or {}).items()))]
    if unconfined:
        lines.append(f"- **unconfined: {', '.join(unconfined)}** — these ran on the host "
                     "with no sandbox and could write outside the run directory")

    lines += ["\n## Summary (infrastructure failures excluded)\n",
          ("| harness | condition | pass | judge | failures | per 10 calls | edits "
           "| turns | tokens | sec | infra |"),
          "|---|---|---|---|---|---|---|---|---|---|---|"]
    for h in harnesses:
        for cond in ("control", "patch"):
            if not (m := agg.get((h, cond))):
                continue
            rate = "—" if m["rate"] is None else f"{m['rate']:.1f}"
            judge = "—" if m["judge"] is None else f"{m['judge']:.1f}"
            lines.append(
                f"| {h} | {cond} | {m['pass']}/{m['n_clean']} | {judge} | "
                f"{_num(m['fails'])} | {rate} | {_num(m['edits'])} | {_num(m['turns'])} | "
                f"{_tok(m['tokens'])} | {_num(m['sec'])} | {m['infra']} |")

    blockers = []
    if runs < MIN_RUNS_FOR_VERDICT:
        blockers.append(f"only {runs} run(s) per combination; behaviour is stochastic, 3+ needed")
    if env.get("net_pypi") != "200":
        blockers.append(f"network to pypi returned `{env.get('net_pypi')}`; "
                        "dependencies cannot be installed")
    # A contaminated host taints the harnesses that ran there, not the whole
    # experiment: devin can never be containerised, so a global blocker here
    # would mean this machine never produces a verdict again. Only when every
    # harness ran on the host is there nothing clean left to decide from.
    contaminated = bool(env.get("system_has_matplotlib") or env.get("system_has_pandas"))
    tainted = set(hosted) if contaminated else set()
    if tainted and tainted >= set(harnesses):
        blockers.append(
            "required packages are already in the host python and every harness "
            "ran on the host - a patch forbidding the system interpreter "
            "competes on unequal terms")
    # One harness with broken infrastructure (auth, network, a dead CLI) is
    # excluded by name below, like a contaminated one — it must not veto the
    # harnesses that measured cleanly. Global blockers are for global causes.
    broken = {h for h in harnesses
              if not (agg.get((h, "control")) or {}).get("n_clean")
              or not (agg.get((h, "patch")) or {}).get("n_clean")}

    # The negative control has to fail. A patch is only ever shown to help by
    # comparison with a baseline that exhibits the problem, so a control passing
    # everywhere means the fixture never reproduced it — and then "the patch
    # changes nothing" is a statement about the fixture, not about the patch.
    controls = [agg[(h, "control")] for h in harnesses if agg.get((h, "control"))]
    if controls and all(c["n_clean"] and c["pass"] == c["n_clean"] for c in controls):
        blockers.append(
            "the control passed in every harness: the fixture does not reproduce the "
            "failure, so there is no baseline to improve on. Make the task actually "
            "trigger it, or check that verify.py tests the failure and not something else")

    # And the mirror case. Nothing passing anywhere reads as "the patch does not
    # help", which is indistinguishable from a verify.py that checks the wrong
    # thing — a real run failed all 18 while every agent had solved the task,
    # because the check tripped over untracked files the arena itself created.
    clean_rows = [r for r in rows if not r.get("infra_failure")]
    if clean_rows and not any(r.get("verdict") == "pass" for r in clean_rows):
        blockers.append(
            f"not one of {len(clean_rows)} runs passed verify.py: either the task is "
            "unsolvable as written or the check tests the wrong thing. Run verify.py "
            "by hand against a working copy before believing this")

    lines.append("\n## Verdict\n")
    if blockers:
        lines.append("**Not enough data for a verdict.** Reasons:\n")
        lines += [f"- {b}" for b in blockers]
        lines.append("\nFix the above and re-run. A verdict on contaminated data is worse "
                 "than no verdict, because it looks like a result.")
    else:
        verdicts = []
        for h in harnesses:
            if h in broken:
                lines.append(f"- **{h}** — excluded: no clean runs in one of the "
                             "conditions (infrastructure or auth broke them — "
                             "see the markers in its rows)")
                verdicts.append("excluded")
                continue
            c, p = agg[(h, "control")], agg[(h, "patch")]
            dpass = p["pass"] / p["n_clean"] - c["pass"] / c["n_clean"]
            drate = (p["rate"] or 0) - (c["rate"] or 0)
            dstuck = (p["stuck"] or 0) - (c["stuck"] or 0)
            # fewer failures on less work means "gave up earlier", not improvement
            gave_up = dpass < 0 and (p["edits"] or 0) < (c["edits"] or 0)  # did less, not better
            if h in tainted:
                v = ("excluded (ran on the host, where the packages are "
                     "preinstalled - unequal terms)")
            elif c["pass"] == c["n_clean"]:
                # This harness never showed the problem, so it cannot show a fix
                # either. Reported per harness rather than as a global blocker: one
                # unrepresentative harness must not mask the ones that did reproduce it.
                v = "no baseline (control passed every run — nothing to improve on here)"
            elif dpass > 0 or (dpass == 0 and (drate < 0 or dstuck < 0)):
                v = "profit"
            elif dpass < 0 or drate > 0 or dstuck > 0:
                v = "harm" + (" (with the patch the agent did less and gave up earlier)"
                              if gave_up else "")
            else:
                v = "zero"
            verdicts.append(v.split()[0])
            rate_c = "—" if c["rate"] is None else f"{c['rate']:.1f}"
            rate_p = "—" if p["rate"] is None else f"{p['rate']:.1f}"
            lines.append(
                f"- **{h}** — {v}: pass {c['pass']}/{c['n_clean']} → "
                f"{p['pass']}/{p['n_clean']}, failures per 10 calls "
                f"{rate_c} -> {rate_p}, "
                f"edits {_num(c['edits'])} -> {_num(p['edits'])}")
        lines.append("")
        # Only harnesses whose control reproduced the failure, in a clean
        # environment, can carry the overall verdict; the rest measured nothing
        # and must not be averaged in.
        scored = [v for v in verdicts if v not in ("no", "excluded")]
        skipped = len(verdicts) - len(scored)
        if not scored:
            lines.append(
                "**No verdict: nothing measurable survived.** Every harness was "
                "excluded — no baseline, a contaminated host, or no clean runs; "
                "the per-harness lines above say which.")
        elif all(v == "profit" for v in scored):
            lines.append("**The patch works** on every harness tested - ship it.")
        elif "harm" in scored and "profit" in scored:
            lines.append("**The patch diverges across harnesses.** "
                         "Do not ship until the cause is understood.")
        elif "harm" in scored:
            lines.append(
                "**The patch harms.** Do not ship. Common cause: the model cannot "
                "satisfy the rule and works around it by weakening the constraint.")
        elif "profit" in scored:
            # profit mixed with zero and no harm anywhere — the one combination
            # the branches above miss, and a real run hit it: the report ended
            # with four bullets and no conclusion.
            lines.append(
                "**The patch helps some harnesses and changes nothing for the rest.** "
                "No harm measured anywhere. Ship if the helped harnesses matter; "
                "where it changed nothing, it still costs the context it occupies.")
        elif all(v == "zero" for v in scored):
            lines.append("**The patch changes nothing** - wasted context in "
                         "every session. Do not ship.")
        if scored and skipped:
            lines.append(f"\nBased on {len(scored)} of {len(verdicts)} harnesses: "
                         f"{skipped} excluded (no baseline, or a contaminated host).")

    # The judge ranks quality; verify stays the ground truth. Their disagreement
    # is a finding about one of them, so it is listed, not averaged away.
    judged = [r for r in rows if r.get("judge")]
    if judged:
        lines.append(f"\n## Judge (blind, model: `{env.get('judge')}`)\n")
        lines.append(
            "Scores grade HOW WELL THE TASK AS WRITTEN was executed — the judge "
            "is deliberately blind to what the fixture measures, so a run can "
            "score 10/10 and still fail the experiment (verify.py alone knows "
            "the goal and alone decides the verdict). Read a high score beside "
            "a fail as a finding, not a contradiction.")
        ranking = sorted(
            ((h, agg[(h, "patch")]["judge"]) for h in harnesses
             if agg.get((h, "patch")) and agg[(h, "patch")]["judge"] is not None),
            key=lambda x: -x[1])
        if ranking:
            lines.append("Patch-condition quality, best first: "
                         + ", ".join(f"**{h}** {s:.1f}" for h, s in ranking) + ".")
        disagree = [r for r in judged
                    if r["judge"]["solved"] != (r["verdict"] == "pass")]
        if disagree:
            lines.append("\nJudge vs verify disagreements (one of them is wrong — look):\n")
            lines += [f"- `{r['harness']}/{r['condition']}#{r['run']}` — verify "
                      f"{r['verdict']}, judge "
                      f"{'solved' if r['judge']['solved'] else 'not solved'}: "
                      f"{r['judge']['note']}" for r in disagree]
        flagged = [r for r in judged if r["judge"]["collateral"]]
        if flagged:
            lines.append("\nCollateral noted by the judge:\n")
            lines += [f"- `{r['harness']}/{r['condition']}#{r['run']}` — "
                      + "; ".join(r["judge"]["collateral"]) for r in flagged]
        fabricated = [r for r in judged if r["judge"].get("unsupported_claims")]
        if fabricated:
            lines.append("\nClaims the tree and the trace do not support:\n")
            lines += [f"- `{r['harness']}/{r['condition']}#{r['run']}` — "
                      + "; ".join(r["judge"]["unsupported_claims"]) for r in fabricated]

    if infra_total:
        lines.append("\n## Infrastructure failures\n")
        lines += [f"- `{r['harness']}/{r['condition']}#{r['run']}` — "
                  f"{', '.join(r.get('infra_hits') or [])}"
                  for r in rows if r.get("infra_failure")]

    stuck_all: Counter[str] = Counter()
    for r in rows:
        for t, n in (r.get("stuck_targets") or {}).items():
            stuck_all[t] += n
    if stuck_all:
        lines.append("\n## Stuck targets (3+ failures on one target)\n")
        lines += [f"- `{t}` — {n}" for t, n in stuck_all.most_common(10)]

    lines += ["\n## All runs\n",
              ("| harness | condition | # | verdict | judge | sandbox | infra | failures "
               "| calls | edits | turns | tokens | sec |"),
              "|---|---|---|---|---|---|---|---|---|---|---|---|---|"]
    for r in sorted(rows, key=lambda r: (r["harness"], r["condition"], r["run"])):
        lines.append(
            f"| {r['harness']} | {r['condition']} | {r['run']} | {r['verdict']} | "
            f"{_judge_cell(r)} | {r.get('sandbox', '?')} | "
            f"{'yes' if r.get('infra_failure') else ''} | {r['tool_failures']} | "
            f"{_num(r['tool_calls'])} | {_num(r['edits'])} | {_num(r.get('turns'))} | "
            f"{_tok(row_tokens(r))} | {r['duration_sec']} |")
    lines.append("\ncalls/edits are hook-traced for "
                 + ", ".join(TRACED)
                 + "; for the rest, edits counts files changed on disk and calls is "
                   "not measured — shown as —, never as 0. tokens follow each "
                   "vendor's own accounting (claude's input excludes cache reads): "
                   "compare within a harness, not across harnesses.")

    text = "\n".join(lines) + "\n"
    out_file.write_text(text, encoding="utf-8")
    return text


# ── placement: who runs where ──────────────────────────────────────────────
def decide_placement(mode: str, only: list[str]) -> tuple[dict[str, str], str]:
    """Which harnesses run in docker and which stay on the host, stated up
    front and recorded — a silent fallback would measure the host while the
    report claims the container."""
    installed = [h for h in HARNESSES if h in only and shutil.which(h)]
    for h in only:
        if h not in installed:
            print(f"  skipped: {h} is not installed")
    if mode == "host":
        return dict.fromkeys(installed, "host"), ""

    if not docker_ready():
        if mode == "docker":
            raise RuntimeError("docker is not available and --sandbox docker was asked for")
        print("  docker unavailable — falling back to host runs (contamination applies)")
        return dict.fromkeys(installed, "host"), ""

    image = ensure_image()
    placement: dict[str, str] = {}
    for h in installed:
        if h in DOCKERLESS:
            placement[h] = "host"
            print(f"  {h}: host — no Linux build of the CLI yet")
            continue
        if seed_auth(h):
            placement[h] = "docker"
        else:
            placement[h] = "host"
            print(f"  {h}: host — no credentials in the container volume. "
                  f"One-time fix: uv run arena.py --docker-login {h}")
    return placement, image


def docker_login(harness: str) -> int:
    """Hand the terminal to an interactive container so the user can log the
    harness in once; the token lands in the named volume and persists."""
    if harness not in HARNESSES:
        print(f"unknown harness: {harness}")
        return 2
    if not docker_ready():
        print("docker is not available")
        return 2
    ensure_image()
    # No "already authenticated" shortcut: the only signal available is file
    # presence, and a stale or host-seeded file passes it while the CLI refuses
    # to run. The user asked for a login — give them the login.
    vol = HOME_VOLUME.format(h=harness)
    if harness == "grok":
        # a host-seeded auth.json is device-bound garbage in the container and
        # makes the login flow report there is nothing to do — clear it first
        subprocess.run(["docker", "run", "--rm", "-v", f"{vol}:/root", DOCKER_IMAGE,
                        "rm", "-f", "/root/.grok/auth.json"],
                       capture_output=True, check=False)
    # grok's own hint when unauthenticated in a browserless container:
    # `grok login --device-code`. Seeding its auth.json from the host is not
    # enough — measured: six 0-second runs, all "Not signed in".
    start = {"claude": ["claude", "/login"], "codex": ["codex", "login"],
             "grok": ["grok", "login", "--device-code"],
             "devin": ["devin", "auth", "login"]}.get(harness, ["bash"])
    print(f"Opening an interactive container. Log in, then exit; the token "
          f"stays in the `{vol}` volume.")
    cmd = ["docker", "run", "--rm", "-it", "-v", f"{vol}:/root", "-w", "/root"]
    if harness == "codex":
        cmd += ["-p", "1455:1455"]  # codex login listens on localhost:1455
    os.execvp("docker", [*cmd, DOCKER_IMAGE, *start])  # noqa: S606
    # Reachable only under test, where execvp is replaced by a mock.
    return 0  # type: ignore[unreachable]


def main() -> int:  # noqa: PLR0911
    ap = argparse.ArgumentParser()
    ap.add_argument("fixture", type=Path, nargs="?")
    ap.add_argument("--runs", type=int, default=1)
    ap.add_argument("--out", type=Path, default=None)
    ap.add_argument("--sandbox", choices=("auto", "docker", "host"), default="auto",
                    help="docker: every run in a container (devin excepted); "
                         "host: the old behaviour, with its contamination")
    ap.add_argument("--only", default=",".join(HARNESSES),
                    help="comma-separated harness subset, e.g. --only codex,claude")
    ap.add_argument("--no-judge", action="store_true",
                    help="skip the blind quality judge")
    ap.add_argument("--judge-all", action="store_true",
                    help="judge every run, not one representative per cell")
    ap.add_argument("--rebuild-image", action="store_true")
    ap.add_argument("--docker-login", metavar="HARNESS",
                    help="one-time interactive login into the harness's container volume")
    a = ap.parse_args()

    if a.docker_login:
        return docker_login(a.docker_login)
    if a.fixture is None:
        ap.error("fixture directory is required")
    if a.rebuild_image:
        ensure_image(rebuild=True)

    fixture = a.fixture.resolve()
    if not fixture.is_dir():
        print(f"not a directory: {fixture}")
        return 2
    for f in REQUIRED:
        if not (fixture / f).exists():
            print(f"incomplete fixture: missing {f}")
            return 2
    # A fixture never stores a file called AGENTS.md. Agents pick those up from
    # unexpected places — one run reported reading /opt/homebrew/AGENTS.md — and
    # patch.md is by definition a rule not yet proven, sometimes a harmful one.
    if (fixture / "AGENTS.md").exists():
        print("fixture must not contain AGENTS.md — use control.md / patch.md")
        return 2
    # Pre-flight: verify.py must FAIL on an empty workdir. A check that passes
    # on nothing can not fail, and a campaign spent on it produces a verdict
    # about nothing. Costs one subprocess; the campaign costs 24 containers.
    with tempfile.TemporaryDirectory() as empty:
        probe = subprocess.run(["uv", "run", str(fixture / "verify.py")],
                               cwd=empty, capture_output=True, text=True,
                               check=False, env={**os.environ, "PATH": host_path()})
    if probe.returncode == 0:
        print("verify.py passes on an EMPTY workdir — it cannot fail, so it "
              "cannot measure. Fix the fixture before spending a campaign.")
        return 2

    only = [h.strip() for h in a.only.split(",") if h.strip()]
    try:
        placement, image = decide_placement(a.sandbox, only)
    except RuntimeError as e:
        print(str(e))
        return 2
    if not placement:
        print("no harnesses to run")
        return 2
    if any(v == "docker" for v in placement.values()):
        sweep_orphan_clones()

    repo = Path.cwd().resolve()
    docker_ctx: dict[str, Any] | None = None
    if "docker" in placement.values():
        try:
            fixture_rel = fixture.relative_to(repo).as_posix()
        except ValueError:
            print(f"docker mode needs the fixture inside the project ({repo}); "
                  f"{fixture} is outside — run from the project root or use --sandbox host")
            return 2
        docker_ctx = {"repo": repo, "fixture_rel": fixture_rel}

    ts = datetime.now(tz=UTC).strftime("%Y%m%d-%H%M%S")
    # Working copies live under .harden/, not in the project root: the arena runs
    # inside whatever repository is being hardened, and a tool that drops hundreds
    # of megabytes beside someone's source tree gets uninstalled, not configured.
    out = (a.out or Path.cwd() / ".harden" / "arena-runs" / ts).resolve()
    out.mkdir(parents=True, exist_ok=True)
    # hook logs live OUTSIDE the run directory: a file changing inside the working
    # folder mid-session gets flagged as modified, and the model reads that as injection
    logdir = Path(tempfile.mkdtemp(prefix=f"arena-{ts}-"))
    (logdir / "clean-settings.json").write_text(json.dumps(CLEAN_SETTINGS),
                                                encoding="utf-8")

    task = (fixture / "task.md").read_text(encoding="utf-8")
    print(f"fixture: {fixture}\nruns per combination: {a.runs}")
    print(f"models: claude={CLAUDE_MODEL}/{EFFORT}  devin={DEVIN_MODEL}  "
          f"codex={CODEX_MODEL}/{EFFORT}  grok={GROK_MODEL}/{EFFORT}")
    print(f"results: {out}\n")

    env = preflight(out, placement, image)
    rows: list[Row] = []
    stop = threading.Event()
    with ThreadPoolExecutor(max_workers=len(placement)) as pool:
        lanes = {pool.submit(
            run_harness_lane, fixture, out, h, a.runs, task, logdir,
            docker_ctx if placement[h] == "docker" else None, stop): h
            for h in placement}
        for fut in as_completed(lanes):
            lane_rows, err = fut.result()
            rows.extend(lane_rows)
            if err is not None:
                print(f"\nfixture is broken, no runs are comparable:\n  {err}")
                return 2

    if not a.no_judge and shutil.which("claude"):
        print()
        judge_all(rows, task, logdir, everything=a.judge_all)

    payload = "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows)
    (out / "results.jsonl").write_text(payload, encoding="utf-8")

    print()
    print(report(rows, env, a.runs, out / "report.md"))

    # Preserve the outcome next to the journal. Working copies stay gitignored —
    # hundreds of megabytes, reproducible from the fixture. The report is kilobytes
    # and NOT reproducible: agents are non-deterministic, so a discarded verdict
    # cannot be re-derived, and `verify: arena` would point at missing evidence.
    keep = Path(os.environ.get("ARENA_KEEP", ".harden/arena")) / f"{ts}-{fixture.name}"
    keep.mkdir(parents=True, exist_ok=True)
    for f in ("report.md", "results.jsonl", "environment.json"):
        if (out / f).exists():
            shutil.copy(out / f, keep / f)
    print(f"saved: {keep}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
