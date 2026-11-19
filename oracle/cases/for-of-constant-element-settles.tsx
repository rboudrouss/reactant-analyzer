// oracle: react=converges
// The element is a constant: the second write stores the value the slot
// holds, and React bails out.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    for (const x of [1, 1]) setN(x);
  });
  return <div>{n}</div>;
}
