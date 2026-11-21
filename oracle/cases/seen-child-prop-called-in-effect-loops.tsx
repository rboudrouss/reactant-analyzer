// oracle: react=loop
// #325, the neighbour of the unseen child: `Child` resolves, and its own
// effect calls the closure it is handed after every commit, writing a new
// object into the parent. The child's regions decide, as before.
import { useEffect, useState } from "react";

function Child({ onPick }: { onPick: (x: object) => void }) {
  useEffect(() => {
    onPick({});
  });
  return null;
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
