// oracle: react=loop
// #335's getter twin: the memo builds the instance once, the render reads a
// getter that calls the setter its constructor kept. The read is a call of
// the getter, in the render (#337).
import { useMemo, useState } from "react";

class K {
  constructor(set) {
    this.set = set;
  }
  get p() {
    this.set((c) => c + 1);
    return 1;
  }
}

export function App() {
  const [n, setN] = useState(0);
  const k = useMemo(() => new K(setN), []);
  const v = k.p;
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
