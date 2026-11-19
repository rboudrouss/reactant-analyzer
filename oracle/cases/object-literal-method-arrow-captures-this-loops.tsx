// oracle: react=loop
// An arrow inside a method reads the method's `this` (#338).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = {
    set: setN,
    run() {
      const go = () => this.set((c: number) => c + 1);
      go();
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
