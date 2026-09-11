"""Validate the status hero's commit-bound required-check snapshot (D-241).

Usage:
    check_status_snapshot.py [--verify-git] [--currency --base SHA --head SHA
        [--max-first-parent-distance N]] <manifest> <evidence_root> <repo_root>

Without flags only the shallow-safe structural validation and the
record-internal invariants run.  ``--verify-git`` additionally proves the
subject side from Git objects (on the first-parent history of HEAD, exactly
one parent, recorded tree); it needs a full-history checkout.  ``--currency`` is the Pages
pull-request leg: when the ``base...head`` range edits the status page or the
status record, the subject must lie on the base tip's first-parent history
and at most N first-parent merges behind it.  A record whose state is ``unavailable`` is
owned by ``check_site_evidence.py`` and passes every mode here unchanged.
"""

import argparse
import json
import sys
from pathlib import Path

import site_status_evidence


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("manifest")
    parser.add_argument("evidence_root")
    parser.add_argument("repo_root")
    parser.add_argument("--verify-git", action="store_true")
    parser.add_argument("--currency", action="store_true")
    parser.add_argument("--base")
    parser.add_argument("--head")
    parser.add_argument("--max-first-parent-distance", type=int, default=20)
    args = parser.parse_args(argv)
    if args.currency and (not args.base or not args.head):
        parser.error("--currency requires --base and --head")
    if args.max_first_parent_distance < 0:
        parser.error("--max-first-parent-distance must be non-negative")
    return args


def load_status_hero(manifest_path):
    try:
        document = json.loads(Path(manifest_path).read_text())
        heroes = document["heroes"]
    except (OSError, ValueError, KeyError, TypeError) as error:
        site_status_evidence.fail(f"cannot read the hero inventory: {error}")
    hero = next((item for item in heroes if isinstance(item, dict) and item.get("page_id") == "status"), None)
    if hero is None:
        site_status_evidence.fail("status hero is missing from the inventory")
    return hero


def main(argv=None):
    args = parse_args(sys.argv[1:] if argv is None else argv)
    hero = load_status_hero(args.manifest)
    evidence_root = Path(args.evidence_root).resolve()
    repo_root = Path(args.repo_root).resolve()
    if hero.get("state") == "unavailable":
        print("status snapshot: hero is unavailable; nothing to verify")
        return 0
    site_status_evidence.validate(hero, evidence_root, repo_root)
    commit = hero["repository"]["commit"]
    if args.verify_git:
        site_status_evidence.verify_git(hero, repo_root)
        print(f"status snapshot: subject {commit} is on the first-parent history of HEAD with one parent and the recorded tree")
    if args.currency:
        if not site_status_evidence.currency_required(repo_root, args.base, args.head):
            print("status snapshot: range does not edit the status page or record; currency not required")
        else:
            distance = site_status_evidence.check_currency(hero, repo_root, args.base, args.max_first_parent_distance)
            print(f"status snapshot: subject {commit} is {distance} first-parent merge(s) behind {args.base}"
                  f" (limit {args.max_first_parent_distance})")
    print(f"status snapshot: record for {commit} is structurally valid ({hero['state']})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
