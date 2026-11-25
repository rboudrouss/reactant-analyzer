// oracle: react=loop
// #412. A fresh ref callback on an element a `.map` callback builds: React
// attaches the new callback at every commit, which stores a new object.
import { useState } from "react";

export function App() {
  const [w, setW] = useState<unknown>(0);
  return (
    <ul>
      {[1, 2].map((i) => (
        <li
          key={i}
          ref={(el) => {
            setW({ el, i });
          }}
        >
          {String(!!w)}
        </li>
      ))}
    </ul>
  );
}
