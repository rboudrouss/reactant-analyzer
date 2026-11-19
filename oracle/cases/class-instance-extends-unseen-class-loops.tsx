// oracle: react=loop
// #335. The parent is a class the analysis does not see (`Map`): the
// instance may hold any member it puts there, and its own method, which the
// render calls, calls the setter its constructor kept.
import { useMemo, useState } from "react";

class K extends Map {
  constructor(set) {
    super();
    this.s = set;
  }
  run() {
    this.s((c) => c + 1);
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
