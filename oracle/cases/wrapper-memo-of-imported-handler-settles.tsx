// @file App.tsx
// oracle: react=converges
// #64. `export const MemoRow = memo(Row)` wraps a component another file
// defines: the element renders that `Row`, resolved as a re-export is, and
// `Row` calls the handed setter only from a click handler nothing fires.
import { useState } from "react";
import { MemoRow } from "./memo-row";

export function App() {
  const [n, setN] = useState(0);
  return (
    <div>
      {n}
      <MemoRow onTick={setN} />
    </div>
  );
}

// @file memo-row.tsx
import { memo } from "react";
import { Row } from "./row";

export const MemoRow = memo(Row);

// @file row.tsx
export function Row({ onTick }: { onTick: (f: (x: number) => number) => void }) {
  return <button onClick={() => onTick((x) => x + 1)}>tick</button>;
}
