// oracle: react=loop
// A method extracted and called with `call`: the receiver is an argument of a
// builtin the analysis does not model, so what it holds escapes there (#338).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    set: setN,
    run() {
      this.set((c: number) => c + 1);
      return 1;
    },
  };
  const f = o.run;
  const v = f.call(o);
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
