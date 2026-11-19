// oracle: react=converges
// The settling neighbour of parent-closure-called-in-child-same-setter-name.tsx:
// the child's own `k` guards a growing effect, and the parent's closure the
// child calls writes the parent's `k`, never the child's (#326). The child's
// `k` stays 5, so `k > 5` never holds; the parent has no effect on its `k`.
import { useEffect, useState } from "react";

function Child({ api }: { api: { f: (x: number) => void } }) {
  const [k, setK] = useState(5);
  useEffect(() => {
    api.f(9);
  }, []);
  useEffect(() => {
    if (k > 5) setK(k + 1);
  }, [k]);
  return <div>{k}</div>;
}

export function App() {
  const [k, setK] = useState(0);
  return (
    <div>
      {k}
      <Child api={{ f: (x) => setK(x) }} />
    </div>
  );
}
