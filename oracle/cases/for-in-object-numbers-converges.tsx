// oracle: react=converges
// #330, the neighbour that settles: `o[k]` under `for…in` reads every member
// of an object of numbers; nothing writes state.
import { useState } from "react";

export function App() {
  const [n] = useState(0);
  const o: Record<string, number> = { a: 1, b: 2 };
  let s = 0;
  for (const k in o) s += o[k];
  return <div>{n}{s}</div>;
}
