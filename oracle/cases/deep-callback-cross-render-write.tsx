// oracle: react=loop
// #267: the child calls its parent's setter three sync callbacks deep in
// render (`cross-setter-in-render`).
import { useState } from "react";

function Child({ onTick, n }: { onTick: (v: number) => void; n: number }) {
  [1].forEach(() => [2].forEach(() => [3].forEach(() => onTick(n + 1))));
  return <div />;
}

export function App() {
  const [n, setN] = useState(0);
  return <Child onTick={setN} n={n} />;
}
