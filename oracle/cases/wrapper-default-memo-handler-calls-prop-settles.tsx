// @file App.tsx
// oracle: react=converges
// #64, the neighbour of `wrapper-default-memo-effect-calls-prop-loops`: the
// memo's function calls the setter it is handed only from a click handler,
// which nothing fires. Seen through the wrapper, the prop keeps its event
// timing; read as an unseen child's (#325) it ran at an unknown time.
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
import { memo } from "react";

function RowImpl({ onTick }: { onTick: (f: (x: number) => number) => void }) {
  return <button onClick={() => onTick((x) => x + 1)}>tick</button>;
}

export default memo(RowImpl);
