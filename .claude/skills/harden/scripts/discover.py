#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Searches skills.sh for an existing skill that covers a failure class.

Used from step 3.5 when the artefact type might be `external-skill`: before
writing a guard, check whether a maintained one already exists. Writing your own
version of something already debugged by a thousand installs is the worst of the
available options.

Install counts are a POPULARITY signal, not a security one. Nothing here decides
whether a skill is safe to install — that is the auditor's job, and it runs on
the full contents, not on a rank.

Usage:
  uv run discover.py "waiting on CI" "polling a pull request"
  uv run discover.py --json "flaky test"
"""
import argparse
import json
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

API = "https://skills.sh/api/search"
PER_QUERY = 10
TIMEOUT_S = 15

# Install-count bands. Coarse on purpose: the difference between 40k and 60k
# installs says nothing, the difference between 200 and 200k does.
TIER_WIDE = 50_000
TIER_KNOWN = 5_000
TIER_SEEN = 500


def tier(installs: int) -> str:
    if installs >= TIER_WIDE:
        return "wide"
    if installs >= TIER_KNOWN:
        return "known"
    if installs >= TIER_SEEN:
        return "seen"
    return "fresh"


def search(query: str, limit: int = PER_QUERY) -> list[dict[str, Any]]:
    url = f"{API}?{urllib.parse.urlencode({'q': query, 'limit': limit})}"
    try:
        with urllib.request.urlopen(url, timeout=TIMEOUT_S) as r:  # noqa: S310
            data = json.loads(r.read())
    except (urllib.error.URLError, OSError, ValueError, json.JSONDecodeError) as e:
        print(f"  ! query {query!r} failed: {e}", file=sys.stderr)
        return []
    return [{**s, "matched": query} for s in data.get("skills", [])]


def rank(batches: list[list[dict[str, Any]]]) -> list[dict[str, Any]]:
    """Merge per-query results, keeping which queries matched each skill.

    Matching several queries is the strongest signal available here: it means the
    skill covers the shape of the problem rather than one keyword in it.
    """
    merged: dict[str, dict[str, Any]] = {}
    for batch in batches:
        for s in batch:
            key = s.get("id") or f"{s.get('source')}/{s.get('name')}"
            if key in merged:
                merged[key]["matched_queries"].add(s["matched"])
            else:
                merged[key] = {
                    "id": key,
                    "name": s.get("name", "?"),
                    "source": s.get("source", "?"),
                    "installs": int(s.get("installs") or 0),
                    "matched_queries": {s["matched"]},
                }
    out = list(merged.values())
    for s in out:
        s["matched_queries"] = sorted(s["matched_queries"])
        s["tier"] = tier(s["installs"])
    out.sort(key=lambda s: (len(s["matched_queries"]), s["installs"]), reverse=True)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("queries", nargs="+", help="describe the failure class, several phrasings")
    ap.add_argument("--limit", type=int, default=PER_QUERY)
    ap.add_argument("--top", type=int, default=10)
    ap.add_argument("--json", action="store_true", dest="as_json")
    a = ap.parse_args()

    results = rank([search(q, a.limit) for q in a.queries])[: a.top]

    if a.as_json:
        print(json.dumps(results, ensure_ascii=False, indent=2))
        return 0

    if not results:
        print("nothing found — write the artefact yourself")
        return 0

    print(f"{len(results)} candidates (install counts are popularity, NOT safety)\n")
    for s in results:
        marks = "★" * len(s["matched_queries"])
        print(f"  {marks:<4} {s['name']:<34} {s['installs']:>8,} installs  [{s['tier']}]")
        print(f"       {s['source']}")
        if len(s["matched_queries"]) > 1:
            print(f"       matched: {', '.join(s['matched_queries'])}")
    print("\nBefore installing any of these, run the auditor: they are executable")
    print("content that will run inside your agent's context.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
