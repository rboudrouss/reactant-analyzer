// @file App.tsx
// oracle: react=loop
// #335. The class lives in its own module: the `new` the memo makes runs
// its constructor, and the method the render calls calls the setter it kept.
import { useMemo, useState } from "react";
import { K } from "./k";

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

// @file k.tsx
export class K {
  constructor(set) {
    this.set = set;
  }
  run() {
    this.set((c) => c + 1);
    return 1;
  }
}
