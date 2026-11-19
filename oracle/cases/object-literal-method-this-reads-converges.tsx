// oracle: react=converges
// The settling twin: the method reads its receiver and writes nothing (#338).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    set: setN,
    k: 2,
    run() {
      return this.k + 1;
    },
  };
  const v = o.run();
  return (
    <div onClick={() => setN((c) => c + 1)}>
      {n}
      {v}
    </div>
  );
}
