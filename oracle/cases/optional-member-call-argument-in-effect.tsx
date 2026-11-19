// oracle: react=converges
// #301. `ref.current` is never attached and stays null: `ref.current?.x(…)`
// skips its argument, the write never runs, and the effect with no deps runs
// once per render without moving `n`. The write in the argument is only a
// possible one.
import { useEffect, useRef, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const ref = useRef<{ x(v: unknown): void } | null>(null);
  useEffect(() => {
    ref.current?.x(setN(n + 1));
  });
  return <div>{n}</div>;
}
