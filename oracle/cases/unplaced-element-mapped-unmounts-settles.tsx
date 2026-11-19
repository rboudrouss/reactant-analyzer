// oracle: react=converges
// #313. `C` is an entry of an array the render maps to elements: the list
// empties after three ticks and the child unmounts.
import { useEffect, useState } from "react";

function C({ onTick }: { onTick: () => void }) {
  const [s, setS] = useState({});
  useEffect(() => {
    setS({});
    onTick();
  });
  return <div>{Object.keys(s).length}</div>;
}

export function App() {
  const [n, setN] = useState(0);
  const items = n < 3 ? [C] : [];
  return (
    <div>
      {items.map((X, i) => (
        <X key={i} onTick={() => setN((x) => x + 1)} />
      ))}
    </div>
  );
}
