// oracle: react=loop
// One function, the method of two literals: each call binds `this` to its own
// receiver, so `b.run()` writes `m` (#338).
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const [m, setM] = useState(0);
  function run(this: { set: (f: (c: number) => number) => void }) {
    this.set((c) => c + 1);
    return 1;
  }
  const a = { set: (_: (c: number) => number) => {}, run };
  const b = { set: setM, run };
  const v = a.run() + b.run();
  return (
    <div>
      {n}
      {m}
      {v}
    </div>
  );
}
