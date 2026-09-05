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
#       --fps-cap 15 --last-frame-duration 3
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
BIN=$ROOT/target/release/reactant
[ -x "$BIN" ] || cargo build --release

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/src/hooks"

cat > "$WORK/src/page.tsx" <<'TSX'
import { useData } from "./hooks/useData";

export function Page() {
  const data = useData(0);
  return <div>{data}</div>;
}
TSX

cat > "$WORK/src/hooks/useData.ts" <<'TS'
import { useState, useEffect } from "react";

export function useData(initial: number) {
  const [value, setValue] = useState(initial);
  useEffect(() => {
    setValue(value + 1);
  }, [value]);
  return value;
}
TS

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

type_run 'npx reactant-analyzer check src/ --trace' "$BIN check src/ --trace || true"
sleep 3.0
SCENE
chmod +x "$WORK/run.sh"

cd "$WORK"
asciinema rec --overwrite --cols 96 --rows 11 -c ./run.sh "$ROOT/docs/demo.cast"
cd "$ROOT"
agg docs/demo.cast docs/demo.gif \
  --theme github-dark --font-size 16 --fps-cap 15 --last-frame-duration 3

echo "wrote docs/demo.cast and docs/demo.gif"
