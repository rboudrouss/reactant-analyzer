// oracle: react=loop
// oracle: known=fn #339
// A method called plainly binds no receiver: its `this` is the global object
// in sloppy code (`undefined` in a module), which the analysis reads as ⊤
// (#338). Here the global object holds the setter, so React loops; the write
// into it is the gap (#339).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  (globalThis as any).set = setN;
  const o = {
    run() {
      this.set((c: number) => c + 1);
      return 1;
    },
  };
  const f = o.run as () => number;
  const v = f();
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
