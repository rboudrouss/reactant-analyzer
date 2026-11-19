// oracle: react=loop
// #335. `K` declares no constructor: the language's hands every argument to
// `B`'s, which keeps the setter; the method the render calls is `B`'s.
import { useMemo, useState } from "react";

class B {
  constructor(set) {
    this.s = set;
  }
  run() {
    this.s((c) => c + 1);
    return 1;
  }
}

class K extends B {}

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
