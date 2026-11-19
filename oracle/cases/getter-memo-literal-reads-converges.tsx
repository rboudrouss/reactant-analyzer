// oracle: react=converges
// The settling twin: the getter the render reads reads state and writes
// nothing (#337).
import { useMemo, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = useMemo(
    () => ({
      k: 2,
      get p() {
        return this.k + 1;
      },
    }),
    [],
  );
  const v = o.p;
  return (
    <div onClick={() => setN((c) => c + 1)}>
      {n}
      {v}
    </div>
  );
}
