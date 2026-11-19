// oracle: react=converges
// #335, the twin: the constructor keeps the setter, the method the render
// calls never calls it.
import { useMemo, useState } from "react";

class K {
  constructor(set) {
    this.set = set;
  }
  run() {
    return 1;
  }
}

export function App() {
  const [n, setN] = useState(0);
  const k = useMemo(() => new K(setN), []);
  const v = k.run();
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
