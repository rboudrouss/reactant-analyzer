#!/usr/bin/env python3
"""Golden-file gate for the corpus measure (#15).

``corpus-diff.py`` compares two runs you happen to have. This compares a run
against a **committed** baseline, so the number in the repository is the one the
tool produced and a change that moves it cannot land quietly.

Why it exists: on 2026-09-03 the precision log's ``1 332 -> 1 314`` turned out to
be ``1 325`` when the frozen binary was re-run. The entry had written its
endpoint by subtracting measured removals from the start point instead of
counting the after-run. The analysis is deterministic at the byte level, so this
was not drift — it was a hand-written number. A hand-written number is the
failure mode; this file is what removes the opportunity.

    scripts/corpus-baseline.py run.json                 # compare, exit 1 on drift
    scripts/corpus-baseline.py run.json --generate      # (re)write the baseline
    scripts/corpus-baseline.py run.json --show 20       # list what moved

The baseline records the corpus fingerprint alongside the counts. Comparing
runs from two different corpora is meaningless, so a fingerprint mismatch is
refused rather than reported as a delta.
"""

import argparse
import hashlib
import json
import subprocess
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASELINE = ROOT / "docs" / "corpus-baseline.json"


def locations(path):
    """Distinct (file, line, col, message) tuples -> rule. The comparable unit."""
    with open(path) as fh:
        diags = json.load(fh)["diagnostics"]
    return {(d["file"], d["line"], d["col"], d["message"]): d["rule"] for d in diags}


def repo_of(file):
    """`test-repo/<name>/...` -> `<name>`; anything else -> `(other)`."""
    if not file:
        return "(unlocated)"
    parts = Path(file).parts
    if len(parts) >= 2 and parts[0] == "test-repo":
        return parts[1]
    return "(other)"


def digest(index):
    """Exact-match hash over the sorted location list.

    The counts alone would let an equal number of removals and additions pass
    unnoticed — which is exactly the shape the 2026-09-03 error took.
    """
    h = hashlib.sha256()
    for key in sorted(index, key=lambda k: (str(k[0]), k[1] or 0, k[2] or 0, k[3])):
        h.update(f"{key[0]}|{key[1]}|{key[2]}|{key[3]}\n".encode())
    return h.hexdigest()


def fingerprint():
    """Corpus identity, from the pinned manifest. `None` when unverifiable."""
    try:
        out = subprocess.run(
            [str(ROOT / "scripts" / "setup-test-repo.sh"), "--verify"],
            capture_output=True,
            text=True,
            timeout=120,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    lines = [ln.strip() for ln in out.stdout.splitlines() if ln.strip()]
    if not lines or any(" ok" not in ln for ln in lines):
        return None
    return hashlib.sha256("\n".join(sorted(lines)).encode()).hexdigest()[:16]


def summarize(index):
    return {
        "total": len(index),
        "digest": digest(index),
        "by_rule": dict(sorted(Counter(index.values()).items())),
        "by_repo": dict(sorted(Counter(repo_of(k[0]) for k in index).items())),
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("run", help="JSON from `reactant --format json test-repo`")
    p.add_argument("--generate", action="store_true", help="(re)write the baseline")
    p.add_argument("--show", type=int, default=0, metavar="N", help="list N moved locations")
    args = p.parse_args()

    index = locations(args.run)
    summary = summarize(index)
    fp = fingerprint()
    summary["corpus"] = fp or "unverified"

    if args.generate:
        BASELINE.write_text(json.dumps(summary, indent=2) + "\n")
        print(f"wrote {BASELINE.relative_to(ROOT)}: {summary['total']} locations")
        if fp is None:
            print(
                "NOTE: corpus unverified — run scripts/setup-test-repo.sh --verify.\n"
                "      A baseline from an unidentifiable corpus only gates THIS machine.",
                file=sys.stderr,
            )
        return 0

    if not BASELINE.exists():
        print(f"no baseline at {BASELINE.relative_to(ROOT)} — run with --generate", file=sys.stderr)
        return 2
    base = json.loads(BASELINE.read_text())

    if base.get("corpus") != summary["corpus"]:
        print(
            f"corpus mismatch: baseline={base.get('corpus')} run={summary['corpus']}\n"
            "Comparing runs from two different corpora is meaningless. Re-clone with\n"
            "scripts/setup-test-repo.sh --force, or regenerate the baseline.",
            file=sys.stderr,
        )
        return 2

    if base["digest"] == summary["digest"]:
        print(f"corpus unchanged: {summary['total']} distinct locations")
        return 0

    print(f"baseline: {base['total']}")
    print(f"run:      {summary['total']}")
    print(f"delta:    {summary['total'] - base['total']:+d}")

    for label, key in (("by rule", "by_rule"), ("by repo", "by_repo")):
        moved = {
            k: (summary[key].get(k, 0) - base[key].get(k, 0))
            for k in set(base[key]) | set(summary[key])
        }
        moved = {k: v for k, v in moved.items() if v}
        if moved:
            print(f"\n{label}:")
            for k, v in sorted(moved.items(), key=lambda kv: -abs(kv[1])):
                print(f"  {v:+5d}  {k}")

    if args.show:
        print(
            f"\nDigest changed. For the moved locations themselves, keep the previous\n"
            f"run and use: scripts/corpus-diff.py before.json {args.run} --show {args.show}"
        )

    print(
        "\nThe corpus moved. If that is the point of the change, regenerate with\n"
        "  scripts/corpus-baseline.py <run.json> --generate\n"
        "and say in the commit message what moved and why.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
