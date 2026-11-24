#!/usr/bin/env python3
"""Merge per-repo analyzer runs into one corpus run.

    scripts/corpus-merge.py out.json run-a.json run-b.json ...

Each repo of the corpus is its own project and is analyzed in its own process
(its own import scopes, its own memory): the whole tree in one process is not
a project, and it does not fit a CI runner. The merged file keeps the shape
`corpus-baseline.py` and `corpus-diff.py` read: `diagnostics` concatenated,
`files_analyzed` summed, `parse_errors` and `blind_spots` concatenated, and the
numeric fields of `summary` summed. Each run's own `summary` is kept under
`summaries`, keyed by the run file's name (the repo's): its counts of the Info
rows the run did not list are per repo, which `corpus-baseline.py` records.
"""

import json
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    out, runs = sys.argv[1], sys.argv[2:]
    merged = None
    summaries = {}
    for path in runs:
        with open(path) as fh:
            run = json.load(fh)
        summaries[Path(path).stem] = dict(run.get("summary", {}))
        if merged is None:
            merged = run
            continue
        if run.get("version") != merged.get("version"):
            print(f"{path}: version {run.get('version')} != {merged.get('version')}", file=sys.stderr)
            return 1
        merged["files_analyzed"] += run.get("files_analyzed", 0)
        for key in ("parse_errors", "diagnostics", "blind_spots"):
            merged.setdefault(key, []).extend(run.get(key, []))
        for key, value in run.get("summary", {}).items():
            if isinstance(value, (int, float)) and not isinstance(value, bool):
                merged["summary"][key] = merged["summary"].get(key, 0) + value
    if merged is None:
        return 2
    merged["summaries"] = summaries
    with open(out, "w") as fh:
        json.dump(merged, fh)
    print(f"{out}: {len(runs)} runs, {len(merged['diagnostics'])} diagnostics")
    return 0


if __name__ == "__main__":
    sys.exit(main())
