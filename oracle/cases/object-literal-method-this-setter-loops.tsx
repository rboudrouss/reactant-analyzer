// oracle: react=loop
// oracle: known=fn #338
// `this` in an object literal's method is the receiver of the call, which
// the analysis does not bind: the call through `this.set` runs nothing.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    set: setN,
    run() {
      this.set((c) => c + 1);
      return 1;
    },
  };
  const v = o.run();
  return (
    <div>
      {n}
      {v}
    </div>
  );
}
