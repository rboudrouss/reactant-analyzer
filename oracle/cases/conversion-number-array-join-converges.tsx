// oracle: react=converges
// #360: `join` of an array of numbers converts numbers: nothing of the
// program runs, and the setter is only the click handler's.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const s = [1, 2, n].join("-");
  return <div onClick={() => setN(n + 1)}>{s}</div>;
}
