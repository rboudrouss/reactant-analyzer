// oracle: react=loop
// The render spelling of `generator-yields-setter-loops` (#317).
import { useState } from "react";

function* gen(s: (v: number) => void) {
  yield s;
}

export function App() {
  const [n, setN] = useState(0);
  for (const f of gen(setN)) f(n + 1);
  return <div>{n}</div>;
}
