// oracle: react=loop
// #216: every render passes `() => setN(n + 1)`, the same kind of reference
// over a new `n`. The child cache compared the props' values alone, so the
// child's analysis under the first `n` answered for every later one, the
// write never grew past `1`, and the loop read as converging.
import { useEffect, useState } from "react";

function Child({ tick }: { tick: () => void }) {
  useEffect(() => {
    tick();
  });
  return null;
}

export function App() {
  const [n, setN] = useState(0);
  return <Child tick={() => setN(n + 1)} />;
}
