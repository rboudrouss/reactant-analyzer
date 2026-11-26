// oracle: react=loop
// #421: one prop holds two closures at two elements, and the child's effect
// calls it. The second closure steps the parent's slot, which the element's
// `p1` is computed from: a loop. The writer row of `on0(p1)` stands for both
// closures' writes and stores the join of their values, not the first
// closure's `0` alone, so the churn graph has the edge without the
// cross-component arm.
import { useEffect, useState } from "react";

function C0({ on0, p1 }: { on0: (x: number) => void; p1: number }) {
  useEffect(() => {
    on0(p1);
  }, [p1]);
  return null;
}

export function App() {
  const [s, setS] = useState(0);
  return (
    <div>
      <C0 on0={() => setS(0)} p1={1} />
      <C0 on0={(x) => setS(x + 2)} p1={s + 2} />
    </div>
  );
}
