// oracle: react=loop
// #270: the same, the child storing the closure from an effect.
import { useEffect, useRef, useState } from "react";

function Child({ cbRef, n, onTick }: any) {
  useEffect(() => {
    cbRef.current = () => onTick(n + 1);
  });
  return <div>{n}</div>;
}

export function App() {
  const [n, setN] = useState(0);
  const ref = useRef<() => void>(() => {});
  useEffect(() => {
    ref.current();
  });
  return <Child cbRef={ref} n={n} onTick={setN} />;
}
