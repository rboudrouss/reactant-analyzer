// @file App.tsx
// oracle: react=converges
// #64. `export default memo(RowImpl)`: `RowImpl`'s effect calls the setter
// the parent hands it, once; the parent re-renders, hands the same setter,
// and the memo skips the child's render, so the effect does not run again.
// The analysis reads `memo(C)` as `C`, a skipped render being a sub-case of
// one it runs: a Warning is a tolerated false positive, an Error would not be.
import { useState } from "react";
import Row from "./row";

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <Row onTick={setN} />
    </div>
  );
}

// @file row.tsx
import { memo, useEffect } from "react";

function RowImpl({ onTick }: { onTick: (f: (x: number) => number) => void }) {
  useEffect(() => {
    onTick((x) => x + 1);
  });
  return null;
}

export default memo(RowImpl);
