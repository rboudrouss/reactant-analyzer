// oracle: react=converges
// The neighbour of `generator-yields-setter-loops`: what the generator yields
// is a constant, so the write settles. The analysis reads a yielded value as
// unknown (#317) and may warn.
import { useEffect, useState } from "react";

function* gen() {
  yield 0;
}

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    for (const x of gen()) setN(x);
  });
  return <div>{n}</div>;
}
