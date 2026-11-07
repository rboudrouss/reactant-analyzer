// oracle: react=loop
// #241: the same forwarding chain; the child's effect on `items` writes the
// grandparent's state, and the grandparent passes a fresh array at one site:
// every commit re-runs the effect.
import { useEffect, useState } from "react";

function Child({ items, onSeen }: { items: number[]; onSeen: (n: number) => void }) {
  useEffect(() => {
    onSeen(items.length);
  }, [items]);
  return <div>{items.length}</div>;
}

function Parent({ items, onSeen }: { items: number[]; onSeen: (n: number) => void }) {
  return <Child items={items} onSeen={onSeen} />;
}

const STABLE = [1];

export function App() {
  const [seen, setSeen] = useState(0);
  return (
    <div>
      <Parent items={[seen, 1]} onSeen={(n) => setSeen(seen + n)} />
      <Parent items={STABLE} onSeen={() => {}} />
    </div>
  );
}
