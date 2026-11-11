// oracle: react=converges
// #241: `Parent` forwards `items` unchanged; one grandparent site passes a
// fresh array, another a module constant. Through the fresh site the child's
// memo re-runs at every render (`always-unstable-deps` names `App`); React
// settles, which is what the oracle judges.
import { useMemo } from "react";

const STABLE = [1, 2];

function Child({ items }: { items: number[] }) {
  const total = useMemo(() => items.reduce((a, b) => a + b, 0), [items]);
  return <div>{total}</div>;
}

function Parent({ items }: { items: number[] }) {
  return <Child items={items} />;
}

export function App() {
  return (
    <div>
      <Parent items={[1, 2, 3]} />
      <Parent items={STABLE} />
    </div>
  );
}
