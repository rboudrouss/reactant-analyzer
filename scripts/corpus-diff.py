#!/usr/bin/env python3
"""Compare two corpus runs by distinct location.

The precision log's comparable column is the number of distinct
``(file, line, col, message)`` tuples across ``diagnostics``. The JSON keeps one
row per (finding, component) — #129 groups only at display — so row counts are
not a clean series.

Why this exists: on 2026-09-03 an entry wrote its endpoint by subtracting
measured removals from the start point instead of counting the after-run, and
the number was wrong by 11. A delta says nothing until the *additions* are
counted too, and the column is cumulative, so one bad subtraction moves every
row after it. Print both sides, always.

    scripts/corpus-diff.py before.json after.json
    scripts/corpus-diff.py after.json            # just count one run
    scripts/corpus-diff.py before.json after.json --show 20
"""

import argparse
import json
import sys
from collections import Counter


def locations(path):
    """Distinct (file, line, col, message) tuples, keyed for reporting by rule."""
    with open(path) as fh:
        diags = json.load(fh)["diagnostics"]
    return {
        (d["file"], d["line"], d["col"], d["message"]): d["rule"] for d in diags
    }


def by_rule(keys, index):
    return Counter(index[k] for k in keys).most_common()


def sort_key(key):
    """Order locations without assuming they have one.

    A finding whose witness chain names no source range carries `line: null`
    (limitations.md, "Every finding carries a position", residual #131), and
    `None < int` raises. Same key `corpus-baseline.py::digest` already uses, so
    the two scripts order the corpus identically.
    """
    f, line, col, msg = key
    return (str(f), line or 0, col or 0, msg)


def fmt(label, keys, index, show):
    if not keys:
        return
    print(f"\n{label} ({len(keys)}):")
    for rule, n in by_rule(keys, index):
        print(f"  {n:5d}  {rule}")
    for key in sorted(keys, key=sort_key)[:show]:
        f, line, col, msg = key
        print(f"    {f}:{line}:{col}  {msg[:100]}")
    if show and len(keys) > show:
        print(f"    … {len(keys) - show} more")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("before")
    p.add_argument("after", nargs="?")
    p.add_argument(
        "--show",
        type=int,
        default=0,
        metavar="N",
        help="also list the first N locations on each side",
    )
    args = p.parse_args()

    if args.after is None:
        index = locations(args.before)
        print(f"{len(index)} distinct locations")
        for rule, n in by_rule(index, index):
            print(f"  {n:5d}  {rule}")
        return 0

    before, after = locations(args.before), locations(args.after)
    removed, added = set(before) - set(after), set(after) - set(before)

    print(f"before: {len(before)}")
    print(f"after:  {len(after)}")
    print(f"delta:  {len(after) - len(before):+d}  ({len(removed)} removed, {len(added)} added)")
    fmt("REMOVED", removed, before, args.show)
    fmt("ADDED", added, after, args.show)

    # The endpoint is `after`, counted. Never `before` minus removals.
    if len(before) - len(removed) + len(added) != len(after):
        print("\nBUG: counts do not reconcile", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
