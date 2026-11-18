// oracle: react=loop
// The renamed twin of parent-closure-called-in-child-same-setter-name.tsx: the
// child's pair is `[j, setJ]`, so no name of the child shadows the closure's.
// The parent's `k` takes 9 and arms the parent's growing effect.
import { useEffect, useState } from "react";

function Child({ api }: { api: { f: (x: number) => void } }) {
  const [j, setJ] = useState(5);
  useEffect(() => {
    api.f(9);
  }, []);
  return <div>{j}</div>;
}

export function App() {
  const [k, setK] = useState(0);
  useEffect(() => {
    if (k > 0) setK(k + 1);
  }, [k]);
  return (
    <div>
      {k}
      <Child api={{ f: (x) => setK(x) }} />
    </div>
  );
}
