// oracle: react=converges
// #270 negative: the child stores a growing closure in the parent's ref,
// and nothing ever calls it.
import { useRef, useState } from "react";

function Child({ cbRef, n, onTick }: any) {
  cbRef.current = () => onTick(n + 1);
  return <div>{n}</div>;
}

export function App() {
  const [n, setN] = useState(0);
  const ref = useRef<() => void>(() => {});
  return <Child cbRef={ref} n={n} onTick={setN} />;
}
