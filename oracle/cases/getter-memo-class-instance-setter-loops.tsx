// oracle: react=loop
// oracle: known=fn #337
// #335's getter twin: the memo builds the instance once, the render reads a
// getter that calls the setter its constructor kept. The getter's write is
// placed where the instance is built, in a memo that runs once.
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
