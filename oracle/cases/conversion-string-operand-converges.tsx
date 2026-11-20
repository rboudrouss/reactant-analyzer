// oracle: react=converges
// #360: a string operand converts to itself: nothing runs, and the setter is
// only the click handler's.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const [t] = useState("a");
  const s = `${t}` + t + -t + (t < "b" ? 1 : 0);
  return <div onClick={() => setN(n + 1)}>{n}{s}</div>;
}
