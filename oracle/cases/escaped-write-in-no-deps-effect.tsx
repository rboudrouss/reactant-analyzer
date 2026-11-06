// oracle: react=loop
// found by: oracle fuzz --seed 3, program 944 (fn)
// React loops (render-budget) through C1; no loop claim on them; claims only outside the loop (warning cross-component-infinite-loop on C0)
// #263: C1's self-edge sits in one strongly connected component with
// App's `s0` (C0, which C1 also renders, writes it), and the graph gave each
// component one cycle: the one through C0's effect, none through C1's.
import { useState, useEffect } from "react";
import { runNow } from "ext";

function useSlot(init) {
  const [v, setV] = useState(init);
  return [v, setV];
}

export function C0({ on1 }) {
  useEffect(() => {
    return () => {
      on1(0);
    };
  });
  return <div />;
}

export function C1() {
  const [s0, setS0] = useSlot(2);
  useEffect(() => {
    runNow(() => setS0((prev) => prev + 2));
    return () => {};
  });
  return (
    <div>
      {0 && <C0 />}
    </div>
  );
}

export function App() {
  const [s0, setS0] = useState(2);
  return (
    <div>
      <C0 on1={setS0} />
      <C1 />
    </div>
  );
}
