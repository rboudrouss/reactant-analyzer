// oracle: react=converges
// #335, the twin: the render stores its setter in a module binding, which
// only a click handler calls.
import { useState } from "react";

let setter = null;

export function notify() {
  if (setter) setter((c) => c + 1);
}

export function App() {
  const [n, setN] = useState(0);
  setter = setN;
  return <div onClick={() => notify()}>{n}</div>;
}
