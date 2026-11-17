// oracle: react=converges
// The twin of `await-fresh-dep-loops`: the `await` settles to the same
// object at every render, so the child's effect runs once. The analysis
// reads any awaited value as possibly new (a Warning, tolerated).
import { Suspense, useEffect, useState } from "react";

const D = { v: 1 };
async function load() {
  return D;
}

function Child({ x, bump }: { x: object; bump: () => void }) {
  useEffect(() => {
    bump();
  }, [x]);
  return null;
}

async function Parent({ bump }: { bump: () => void }) {
  const x = await load();
  return <Child x={x} bump={bump} />;
}

export function App() {
  const [n, setN] = useState(0);
  return (
    <Suspense fallback={null}>
      <Parent bump={() => setN(n + 1)} />
    </Suspense>
  );
}
