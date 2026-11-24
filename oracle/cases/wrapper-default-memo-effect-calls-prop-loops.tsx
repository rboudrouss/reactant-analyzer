// @file App.tsx
// oracle: react=loop
// #64. `export default memo(RowImpl)`, imported under another name: the
// element renders `RowImpl`, handed a new closure at every render (so the memo
// renders it again), whose effect calls it after every commit. The child is
// seen, and its body says when the prop runs.
import { useState } from "react";
import Row from "./row";

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <Row onTick={(f) => setN(f)} />
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
