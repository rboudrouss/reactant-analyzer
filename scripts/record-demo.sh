#!/usr/bin/env bash
# Records the README demo and renders it to docs/demo.svg.
#
# Two tools, neither of them a project dependency:
#   pip install asciinema                              (2.4+, records)
#   cargo install --git https://github.com/asciinema/agg   (renders the GIF)
#
# svg-term-cli was tried first and rejected: it animates the first line and
# leaves the rest of the frames in the file but never displays them, so the
# command typed out and the analyzer's output never appeared.
#
# The demo project is generated here rather than committed, so the recording
# has exactly one source of truth and no stray .tsx files sit in the tree.
# Re-render without re-recording:
#   agg docs/demo.cast docs/demo.gif --theme github-dark --font-size 16 \
#       --fps-cap 15 --last-frame-duration 10
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
BIN=$ROOT/target/release/reactant
[ -x "$BIN" ] || cargo build --release

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/src"

cat > "$WORK/src/Dashboard.tsx" <<'TSX'
import { useState } from "react";
import { Filters } from "./Filters";

export function Dashboard() {
  const [query, setQuery] = useState({ term: "", tags: [] });
  return <Filters value={query} onChange={setQuery} />;
}
TSX

cat > "$WORK/src/Filters.tsx" <<'TSX'
import { useEffect } from "react";

export function Filters({ value, onChange }) {
  useEffect(() => {
    onChange({ ...value, term: value.term.trim() });
  }, [value, onChange]);

  return <input value={value.term} />;
}
TSX

cat > "$WORK/run.sh" <<SCENE
#!/usr/bin/env bash
# Types each command a character at a time, then runs it for real. The output
# is the analyzer's, unedited; only the keystrokes are synthetic.
type_run() {
  printf '\033[1;32m\$\033[0m '
  printf '%s' "\$1" | while IFS= read -r -n1 c; do printf '%s' "\$c"; sleep 0.04; done
  printf '\n'
  sleep 0.4
  eval "\$2"
  sleep 2.0
}
sleep 0.7

type_run 'npx reactant-analyzer check src/' "$BIN check src/ || true"
sleep 3.0
SCENE
chmod +x "$WORK/run.sh"

cd "$WORK"
asciinema rec --overwrite --cols 96 --rows 10 -c ./run.sh "$ROOT/docs/demo.cast"
cd "$ROOT"
agg docs/demo.cast docs/demo.gif \
  --theme github-dark --font-size 16 --fps-cap 15 --last-frame-duration 10

echo "wrote docs/demo.cast and docs/demo.gif"
