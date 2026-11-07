// oracle: react=converges
// #270 negative: the closure the child stores writes a constant; the
// parent's effect stores the value already there and React bails out. The
// parent reads the child's closure as a carried setter — its body names the
// child's bindings, so the call's argument is not the stored value — and
// the write may change the slot: a Warning (fp), as for any carried setter.
import { useEffect, useRef, useState } from "react";

function Child({ cbRef, onTick }: any) {
  cbRef.current = () => onTick(1);
  return <div />;
}

export function App() {
  const [n, setN] = useState(0);
  const ref = useRef<() => void>(() => {});
  useEffect(() => {
    ref.current();
  });
  return <Child cbRef={ref} onTick={setN} />;
}
