// oracle: react=converges
// #284, the closure the render does not run: the handler binds `s` to the
// setter and writes through it on a click only, so the render settles.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  return (
    <button
      onClick={() => {
        const s = setN;
        s(n + 1);
      }}
    >
      {n}
    </button>
  );
}
