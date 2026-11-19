// oracle: react=loop
// #335. The memo builds one instance, whose constructor keeps the setter;
// the render calls a method that calls it through `this`: every render
// writes the state, which renders again.
import { useMemo, useState } from "react";

class K {
  constructor(set) {
    this.set = set;
  }
  run() {
    this.set((c) => c + 1);
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
