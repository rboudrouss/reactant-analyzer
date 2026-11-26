// oracle: react=converges
// #412. A fresh ref callback that only stores the node into a ref object
// schedules nothing: React calls it at every commit, and nothing renders
// again.
import { useRef, useState } from "react";

export function App() {
  const [n] = useState(0);
  const box = useRef<HTMLDivElement | null>(null);
  return (
    <div
      ref={(el) => {
        box.current = el;
      }}
    >
      {n}
    </div>
  );
}
