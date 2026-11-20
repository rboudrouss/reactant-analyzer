// oracle: react=loop
// #360: `Array.prototype.join` converts each entry to a string, which calls
// the entry's `toString`, with the setter, at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const o = {
    toString: () => {
      h();
      return "";
    },
  };
  const s = [o].join();
  return <div>{n}{s}</div>;
}
