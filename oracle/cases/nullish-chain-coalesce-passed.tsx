// oracle: react=loop
// #303, the neighbour of `nullish-chain-coalesce-absent`: the parent passes
// `a`, so the chain maps it to a new array at every render, and the effect on
// it bumps the counter without end.
import { useEffect, useState } from "react";

function Child({ a }: { a?: number[] }) {
  const [n, setN] = useState(0);
  const p = a?.map((x) => x + 1) ?? [];
  useEffect(() => {
    setN((c) => c + 1);
  }, [p]);
  return <div>{n}</div>;
}

const ITEMS = [1, 2];

export function App() {
  return <Child a={ITEMS} />;
}
