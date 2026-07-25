#!/usr/bin/env python3
"""Execute deterministic CI checks for the project-local alpha skill evals."""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import re
import signal
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ALPHA_SKILLS = ("pycc", "pycc-feedback")
COMMAND_TIMEOUT_SECONDS = 30
CLIENT_ROOTS = {
    "claude": Path(".claude/skills"),
    "codex": Path(".agents/skills"),
}
BEHAVIORAL_EVIDENCE = Path("tests/alpha_skill_client_evidence.json")
EVIDENCE_SCHEMA_VERSION = 2
CLIENT_EVIDENCE_CONTRACTS = {
    "claude": {
        "version": "Claude Code 2.1.219",
        "entrypoints": ["/pycc", "/pycc-feedback"],
    },
    "codex": {
        "version": "Codex CLI 0.145.0",
        "entrypoints": ["$pycc", "$pycc-feedback"],
    },
}


class EvalCommandTimeout(RuntimeError):
    """Raised when an executable eval exceeds its bounded runtime."""


def has_unnegated_occurrence(haystack: str, phrase: str) -> bool:
    haystack = haystack.replace("’", "'")
    phrase = phrase.replace("’", "'")
    matches = list(re.finditer(re.escape(phrase), haystack))
    if not matches:
        return False
    negators = {
        "never",
        "no",
        "not",
        "cannot",
        "without",
        "forbid",
        "forbidden",
        "refuse",
        "skip",
        "omit",
        "don't",
        "doesn't",
        "didn't",
        "isn't",
        "aren't",
        "wasn't",
        "weren't",
        "won't",
        "wouldn't",
        "shouldn't",
        "couldn't",
        "can't",
        "mustn't",
        "needn't",
    }
    found_unnegated = False
    clause_separators = (".", "!", "?", "\n", ",", ";", ":")
    for match in matches:
        sentence_start = max(
            haystack.rfind(separator, 0, match.start())
            for separator in clause_separators
        )
        sentence_ends = [
            position
            for separator in clause_separators
            if (position := haystack.find(separator, match.end())) >= 0
        ]
        sentence_end = min(sentence_ends, default=len(haystack))
        sentence = haystack[sentence_start + 1 : sentence_end]
        required_despite_negation = (
            "panic" in phrase
            and re.search(
                rf"\bdoes not make\b.*{re.escape(phrase)}.*\bacceptable\b",
                sentence,
            )
        )
        if required_despite_negation:
            found_unnegated = True
            continue
        prefix = haystack[sentence_start + 1 : match.start()]
        suffix = haystack[match.end() : sentence_end]
        words = [
            word.strip("`'\".,:;!?()[]{}")
            for word in (prefix + " " + suffix).split()
        ]
        if negators.intersection(words):
            return False
        found_unnegated = True
    return found_unnegated


def load_evals(root: Path, skill_name: str) -> dict:
    path = root / ".claude" / "skills" / skill_name / "evals" / "evals.json"
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path}: top-level value must be an object")
    return value


def case_contract_failures(
    skill_text: str,
    case: dict,
    label: str,
) -> list[str]:
    failures: list[str] = []
    contract = case.get("contract")
    if not isinstance(contract, dict):
        return [f"{label}: missing machine-checkable contract"]

    expected_output = case.get("expected_output")
    if not isinstance(expected_output, str) or not expected_output.strip():
        failures.append(f"{label}: expected_output must be non-empty text")
        expected_output = ""

    for field, haystack, reject_negation in (
        ("skill_must_contain", skill_text, True),
        ("expected_output_must_contain", expected_output, False),
        ("expected_output_must_require", expected_output, True),
    ):
        phrases = contract.get(field)
        if field == "expected_output_must_require" and phrases is None:
            continue
        if not isinstance(phrases, list) or not phrases:
            failures.append(f"{label}: contract.{field} must be a non-empty list")
            continue
        folded_haystack = " ".join(haystack.casefold().split())
        for phrase in phrases:
            if not isinstance(phrase, str) or not phrase.strip():
                failures.append(f"{label}: contract.{field} has an invalid phrase")
            else:
                folded_phrase = " ".join(phrase.casefold().split())
                if folded_phrase not in folded_haystack:
                    failures.append(
                        f"{label}: {field} phrase {phrase!r} is not enforced"
                    )
                elif (
                    reject_negation
                    and not has_unnegated_occurrence(
                        folded_haystack,
                        folded_phrase,
                    )
                ):
                    failures.append(
                        f"{label}: {field} phrase {phrase!r} appears only "
                        "in a negated context"
                    )
    return failures


def contract_failures(client: str, root: Path = ROOT) -> list[str]:
    failures: list[str] = []
    client_root = root / CLIENT_ROOTS[client]
    for skill_name in ALPHA_SKILLS:
        canonical_path = (
            root / ".claude" / "skills" / skill_name / "SKILL.md"
        )
        client_path = client_root / skill_name / "SKILL.md"
        if not canonical_path.is_file():
            failures.append(f"{canonical_path}: missing canonical skill")
            continue
        if not client_path.is_file():
            failures.append(f"{client_path}: missing {client} entrypoint")
            continue
        canonical_text = canonical_path.read_text(encoding="utf-8")
        if client == "codex":
            wrapper_text = client_path.read_text(encoding="utf-8")
            expected = f".claude/skills/{skill_name}/SKILL.md"
            if expected not in wrapper_text or "canonical workflow" not in wrapper_text:
                failures.append(
                    f"{client_path}: does not load the canonical skill contract"
                )

        try:
            evals = load_evals(root, skill_name)
        except (OSError, json.JSONDecodeError, ValueError) as error:
            failures.append(str(error))
            continue
        cases = evals.get("evals")
        if evals.get("skill_name") != skill_name or not isinstance(cases, list):
            failures.append(f"{skill_name}: invalid eval suite identity")
            continue
        for case in cases:
            if not isinstance(case, dict):
                failures.append(f"{skill_name}: eval case must be an object")
                continue
            label = f"{skill_name} eval {case.get('id', '?')}"
            prompt = case.get("prompt")
            if not isinstance(prompt, str) or not prompt.strip():
                failures.append(f"{label}: prompt must be non-empty text")
            failures.extend(case_contract_failures(canonical_text, case, label))
    return failures


def terminate_process_tree(process: subprocess.Popen[str]) -> None:
    if os.name == "nt":
        subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
        if process.poll() is None:
            process.kill()
        process.wait()
        return

    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    deadline = time.monotonic() + 1
    while time.monotonic() < deadline:
        try:
            os.killpg(process.pid, 0)
        except (PermissionError, ProcessLookupError):
            process.wait()
            return
        time.sleep(0.05)
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except (PermissionError, ProcessLookupError):
        pass
    process.wait()


def run_command(
    command: list[str],
    cwd: Path,
    timeout_seconds: float = COMMAND_TIMEOUT_SECONDS,
) -> subprocess.CompletedProcess[str]:
    popen_arguments: dict = {
        "cwd": cwd,
        "stdout": subprocess.PIPE,
        "stderr": subprocess.PIPE,
        "text": True,
    }
    if os.name == "nt":
        popen_arguments["creationflags"] = (
            subprocess.CREATE_NEW_PROCESS_GROUP
        )
    else:
        popen_arguments["start_new_session"] = True
    process = subprocess.Popen(command, **popen_arguments)
    try:
        stdout, stderr = process.communicate(timeout=timeout_seconds)
        return subprocess.CompletedProcess(
            command,
            process.returncode,
            stdout,
            stderr,
        )
    except subprocess.TimeoutExpired as error:
        terminate_process_tree(process)
        try:
            process.communicate(timeout=2)
        except subprocess.TimeoutExpired:
            for stream in (process.stdout, process.stderr):
                if stream is not None:
                    stream.close()
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=1)
        rendered = " ".join(command)
        raise EvalCommandTimeout(
            f"command timed out after {timeout_seconds:g}s: {rendered}"
        ) from error


def run_runtime_command(
    failures: list[str],
    label: str,
    command: list[str],
    cwd: Path,
) -> subprocess.CompletedProcess[str] | None:
    try:
        return run_command(command, cwd)
    except EvalCommandTimeout as error:
        failures.append(f"{label}: {error}")
        return None


def runtime_failures(
    root: Path = ROOT,
    pycc_bin: Path | None = None,
) -> list[str]:
    failures: list[str] = []
    evals = load_evals(root, "pycc")
    runtime_cases = [
        case
        for case in evals.get("evals", [])
        if isinstance(case, dict) and isinstance(case.get("runtime"), dict)
    ]
    if not runtime_cases:
        return ["pycc evals: no executable runtime case"]

    executable = "pycc.exe" if os.name == "nt" else "pycc"
    if pycc_bin is None:
        pycc_bin = root / "target" / "debug" / executable
    if not pycc_bin.is_file():
        return [
            f"{pycc_bin}: missing compiler; run cargo build --workspace first"
        ]

    for case in runtime_cases:
        runtime = case["runtime"]
        source_text = runtime.get("source")
        expected_stdout = runtime.get("expected_stdout")
        label = f"pycc eval {case.get('id', '?')}"
        if not isinstance(source_text, str) or not isinstance(
            expected_stdout,
            str,
        ):
            failures.append(f"{label}: invalid runtime fixture")
            continue
        with tempfile.TemporaryDirectory(prefix="pycc-skill-eval-") as directory:
            fixture_root = Path(directory)
            source = fixture_root / "program.py"
            output = fixture_root / executable
            source.write_text(source_text, encoding="utf-8")

            build = run_runtime_command(
                failures,
                label,
                [str(pycc_bin), "build", str(source), "-o", str(output)],
                root,
            )
            if build is None:
                continue
            if build.returncode != 0:
                failures.append(
                    f"{label}: build failed with {build.returncode}: {build.stderr}"
                )
                continue
            compiled = run_runtime_command(
                failures,
                label,
                [str(output)],
                root,
            )
            if compiled is None:
                continue
            if compiled.returncode != 0 or compiled.stdout != expected_stdout:
                failures.append(
                    f"{label}: compiled program returned {compiled.returncode}, "
                    f"stdout={compiled.stdout!r}, stderr={compiled.stderr!r}"
                )
            run = run_runtime_command(
                failures,
                label,
                [str(pycc_bin), "run", str(source)],
                root,
            )
            if run is None:
                continue
            if run.returncode != 0 or run.stdout != expected_stdout:
                failures.append(
                    f"{label}: pycc run returned {run.returncode}, "
                    f"stdout={run.stdout!r}, stderr={run.stderr!r}"
                )
    return failures


def feedback_reproduction_failures(
    root: Path = ROOT,
    pycc_bin: Path | None = None,
) -> list[str]:
    failures: list[str] = []
    evals = load_evals(root, "pycc-feedback")
    cases = [
        case
        for case in evals.get("evals", [])
        if isinstance(case, dict)
        and isinstance(case.get("reproduction"), dict)
    ]
    if not cases:
        return ["pycc-feedback evals: no executable reproduction case"]

    executable = "pycc.exe" if os.name == "nt" else "pycc"
    if pycc_bin is None:
        pycc_bin = root / "target" / "debug" / executable
    if not pycc_bin.is_file():
        return [
            f"{pycc_bin}: missing compiler; run cargo build --workspace first"
        ]

    for case in cases:
        reproduction = case["reproduction"]
        source_text = reproduction.get("source")
        expected_exit = reproduction.get("expected_exit")
        stderr_fragment = reproduction.get("stderr_must_contain")
        label = f"pycc-feedback eval {case.get('id', '?')}"
        if (
            not isinstance(source_text, str)
            or not source_text.strip()
            or not isinstance(expected_exit, int)
            or not isinstance(stderr_fragment, str)
            or not stderr_fragment
        ):
            failures.append(f"{label}: invalid reproduction fixture")
            continue
        if source_text not in case.get("prompt", ""):
            failures.append(f"{label}: reproduction source is missing from prompt")
            continue

        with tempfile.TemporaryDirectory(
            prefix="pycc-feedback-eval-"
        ) as directory:
            fixture_root = Path(directory)
            source = fixture_root / "program.py"
            output = fixture_root / executable
            source.write_text(source_text, encoding="utf-8")
            result = run_runtime_command(
                failures,
                label,
                [str(pycc_bin), "build", str(source), "-o", str(output)],
                root,
            )
            if result is None:
                continue
            if (
                result.returncode != expected_exit
                or stderr_fragment not in result.stderr
            ):
                failures.append(
                    f"{label}: reproduction returned {result.returncode}, "
                    f"stdout={result.stdout!r}, stderr={result.stderr!r}"
                )
    return failures


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def tree_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    files = sorted(candidate for candidate in path.rglob("*") if candidate.is_file())
    for candidate in files:
        relative = candidate.relative_to(path).as_posix().encode("utf-8")
        contents = candidate.read_bytes()
        digest.update(struct.pack(">Q", len(relative)))
        digest.update(relative)
        digest.update(struct.pack(">Q", len(contents)))
        digest.update(contents)
    return digest.hexdigest()


def project_input_sha256(root: Path = ROOT) -> str:
    if (root / ".git").exists():
        tracked = subprocess.run(
            ["git", "-C", str(root), "ls-files", "-z"],
            check=True,
            capture_output=True,
            timeout=10,
        ).stdout.split(b"\0")
        files = [
            root / item.decode("utf-8")
            for item in tracked
            if item
            and item.decode("utf-8") != BEHAVIORAL_EVIDENCE.as_posix()
        ]
    else:
        files = [
            candidate
            for candidate in root.rglob("*")
            if candidate.is_file()
            and candidate.relative_to(root) != BEHAVIORAL_EVIDENCE
        ]

    digest = hashlib.sha256()
    for candidate in sorted(files):
        relative = candidate.relative_to(root).as_posix().encode("utf-8")
        contents = candidate.read_bytes()
        digest.update(struct.pack(">Q", len(relative)))
        digest.update(relative)
        digest.update(struct.pack(">Q", len(contents)))
        digest.update(contents)
    return digest.hexdigest()


def required_behavioral_cases(root: Path) -> list[str]:
    required: list[str] = []
    for skill_name in ALPHA_SKILLS:
        evals = load_evals(root, skill_name)
        cases = evals.get("evals")
        if not isinstance(cases, list):
            raise ValueError(f"{skill_name}: evals must be a list")
        for case in cases:
            if not isinstance(case, dict) or not isinstance(case.get("id"), int):
                raise ValueError(f"{skill_name}: every eval needs an integer id")
            required.append(f"{skill_name}:{case['id']}")
    if len(required) != len(set(required)):
        raise ValueError("eval manifests contain duplicate behavioral case ids")
    return required


def behavioral_evidence_requirements(root: Path) -> dict[str, list[str]]:
    requirements: dict[str, list[str]] = {}
    for skill_name in ALPHA_SKILLS:
        evals = load_evals(root, skill_name)
        for case in evals["evals"]:
            case_id = f"{skill_name}:{case['id']}"
            phrases = case.get("evidence_must_contain")
            if (
                not isinstance(phrases, list)
                or not phrases
                or any(
                    not isinstance(phrase, str) or not phrase.strip()
                    for phrase in phrases
                )
            ):
                raise ValueError(
                    f"{case_id}: evidence_must_contain must be non-empty text"
                )
            requirements[case_id] = phrases
    return requirements


def valid_execution_timestamp(value: object) -> bool:
    if not isinstance(value, str):
        return False
    try:
        parsed = datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return False
    if parsed.tzinfo is None:
        return False
    now = datetime.datetime.now(datetime.timezone.utc)
    return parsed <= now + datetime.timedelta(minutes=5)


def behavioral_evidence_failures(root: Path = ROOT) -> list[str]:
    path = root / BEHAVIORAL_EVIDENCE
    try:
        evidence = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return [f"{path}: {error}"]

    failures: list[str] = []
    if evidence.get("schema_version") != EVIDENCE_SCHEMA_VERSION:
        failures.append(
            f"{path}: schema_version must be {EVIDENCE_SCHEMA_VERSION}"
        )
    source_revision = evidence.get("source_revision")
    if not isinstance(source_revision, str) or re.fullmatch(
        r"[0-9a-f]{40}",
        source_revision,
    ) is None:
        failures.append(f"{path}: source_revision must be a full Git commit")
    elif (root / ".git").exists():
        commit_check = subprocess.run(
            [
                "git",
                "-C",
                str(root),
                "merge-base",
                "--is-ancestor",
                source_revision,
                "HEAD",
            ],
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
        if commit_check.returncode != 0:
            failures.append(
                f"{path}: source_revision is not an ancestor of this checkout"
            )
    if not valid_execution_timestamp(evidence.get("executed_at")):
        failures.append(f"{path}: executed_at must be a valid non-future timestamp")
    try:
        expected_project_input = project_input_sha256(root)
    except (OSError, subprocess.SubprocessError, UnicodeDecodeError) as error:
        failures.append(f"{path}: could not fingerprint project input: {error}")
    else:
        if evidence.get("project_input_sha256") != expected_project_input:
            failures.append(
                f"{path}: project_input_sha256 is stale; "
                "rerun both manual client gates"
            )

    artifacts = evidence.get("artifacts")
    if not isinstance(artifacts, dict):
        return [f"{path}: artifacts must be an object"]
    for skill_name in ALPHA_SKILLS:
        artifact = artifacts.get(skill_name)
        if not isinstance(artifact, dict):
            failures.append(f"{path}: missing {skill_name} artifact hashes")
            continue
        expected = {
            "canonical_tree_sha256": tree_sha256(
                root / ".claude" / "skills" / skill_name
            ),
            "claude_entrypoint_sha256": sha256(
                root / ".claude" / "skills" / skill_name / "SKILL.md"
            ),
            "codex_entrypoint_sha256": sha256(
                root / ".agents" / "skills" / skill_name / "SKILL.md"
            ),
        }
        for field, digest in expected.items():
            if artifact.get(field) != digest:
                failures.append(
                    f"{path}: {skill_name} {field} is stale; "
                    "rerun the manual client gate"
                )

    try:
        required_cases = required_behavioral_cases(root)
        evidence_requirements = behavioral_evidence_requirements(root)
    except (OSError, json.JSONDecodeError, ValueError) as error:
        failures.append(str(error))
        required_cases = []
        evidence_requirements = {}

    clients = evidence.get("clients")
    if not isinstance(clients, dict):
        return failures + [f"{path}: clients must be an object"]
    for client, contract in sorted(CLIENT_EVIDENCE_CONTRACTS.items()):
        result = clients.get(client)
        if not isinstance(result, dict):
            failures.append(f"{path}: missing {client} client evidence")
            continue
        if result.get("version") != contract["version"]:
            failures.append(
                f"{path}: {client} version must be {contract['version']}"
            )
        if result.get("entrypoints") != contract["entrypoints"]:
            failures.append(f"{path}: {client} entrypoints do not match the gate")
        if result.get("github_access") != "sanitized-read-only":
            failures.append(
                f"{path}: {client} github_access must be sanitized-read-only"
            )
        if result.get("external_writes") is not False:
            failures.append(f"{path}: {client} external_writes must be false")
        cases = result.get("cases")
        if not isinstance(cases, list):
            failures.append(f"{path}: {client} cases must be a list")
            continue
        indexed: dict[str, dict] = {}
        for case in cases:
            if not isinstance(case, dict) or not isinstance(case.get("id"), str):
                failures.append(f"{path}: {client} has an invalid case record")
                continue
            case_id = case["id"]
            if case_id in indexed:
                failures.append(f"{path}: {client} duplicates {case_id}")
            indexed[case_id] = case
        if set(indexed) != set(required_cases):
            missing = sorted(set(required_cases) - set(indexed))
            extra = sorted(set(indexed) - set(required_cases))
            failures.append(
                f"{path}: {client} case coverage mismatch; "
                f"missing={missing}, extra={extra}"
            )
        for case_id in required_cases:
            case = indexed.get(case_id)
            if not isinstance(case, dict) or case.get("result") != "pass":
                failures.append(
                    f"{path}: {client} {case_id} lacks passing evidence"
                )
            elif (
                not isinstance(case.get("observation"), str)
                or len(case["observation"].split()) < 8
            ):
                failures.append(
                    f"{path}: {client} {case_id} observation is too weak"
                )
            else:
                folded_observation = " ".join(
                    case["observation"].casefold().split()
                )
                for phrase in evidence_requirements.get(case_id, []):
                    folded_phrase = " ".join(phrase.casefold().split())
                    if folded_phrase not in folded_observation:
                        failures.append(
                            f"{path}: {client} {case_id} observation lacks "
                            f"required evidence {phrase!r}"
                        )
    return failures


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--client-entrypoint", choices=sorted(CLIENT_ROOTS))
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--behavioral-evidence", action="store_true")
    args = parser.parse_args()
    if (
        not args.client_entrypoint
        and not args.runtime
        and not args.behavioral_evidence
    ):
        parser.error(
            "select --client-entrypoint, --runtime, or "
            "--behavioral-evidence"
        )
    return args


def main() -> int:
    args = parse_args()
    failures: list[str] = []
    if args.client_entrypoint:
        failures.extend(contract_failures(args.client_entrypoint))
    if args.runtime:
        try:
            failures.extend(runtime_failures())
            failures.extend(feedback_reproduction_failures())
        except (OSError, json.JSONDecodeError, ValueError) as error:
            failures.append(str(error))
    if args.behavioral_evidence:
        failures.extend(behavioral_evidence_failures())
    if failures:
        for failure in failures:
            print(f"error: {failure}", file=sys.stderr)
        return 1
    checked = []
    if args.client_entrypoint:
        checked.append(f"{args.client_entrypoint} entrypoint contracts")
    if args.runtime:
        checked.append("runtime")
    if args.behavioral_evidence:
        checked.append("behavioral evidence")
    print("Alpha skill evals: " + " and ".join(checked) + " valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
