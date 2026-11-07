// oracle: react=loop
// #270: the parent hands its ref to a child, which stores a closure over the
// parent's setter in `current` during render; the parent's effect calls it
// after every commit. The child's write lands in the parent's ref object.
import { useEffect, useRef, useState } from "react";

function Child({ cbRef, n, onTick }: any) {
  cbRef.current = () => onTick(n + 1);
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
