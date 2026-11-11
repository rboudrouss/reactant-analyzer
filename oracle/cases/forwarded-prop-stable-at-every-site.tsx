// oracle: react=converges
// #241 negative: every grandparent site passes a stable array through the
// forwarding parent; no site makes the child's memo re-run.
import { useMemo } from "react";

const A = [1, 2];
const B = [3];

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
      <Parent items={A} />
      <Parent items={B} />
    </div>
  );
}
