// oracle: react=loop
// What an `await` settles to may be new at every render of the component
// that awaits it: a child's effect on it re-runs whenever that component
// re-renders, here because the effect bumps the state above it.
import { Suspense, useEffect, useState } from "react";

async function load() {
  return { v: 1 };
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
