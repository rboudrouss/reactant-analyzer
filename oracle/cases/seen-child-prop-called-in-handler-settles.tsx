// oracle: react=converges
// #325, the neighbour of the unseen child: `Child` resolves, and it calls the
// closure it is handed only from a click handler, which nothing fires. Its own
// regions say so, so the closure stays an event handler.
import { useState } from "react";

function Child({ onPick }: { onPick: (x: object) => void }) {
  return <button onClick={() => onPick({})}>pick</button>;
}

export function App() {
  const [s, setS] = useState<object | number>(0);
  return (
    <div>
      {typeof s}
      <Child onPick={(x: object) => setS(x)} />
    </div>
  );
}
