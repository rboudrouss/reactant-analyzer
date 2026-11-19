// oracle: react=loop
// A getter's `this` is the object read: `o.p` runs the getter with `o`, whose
// `set` is the setter (#338).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    set: setN,
    get p() {
      this.set((c: number) => c + 1);
      return 1;
    },
  };
  const v = o.p;
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
