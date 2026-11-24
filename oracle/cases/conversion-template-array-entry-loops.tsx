// oracle: react=loop
// #360: an array converts to a string through `Array.prototype.toString`,
// which joins its entries, converting each: the entry's `toString` runs, with
// the setter, at every render.
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
  const s = `${[o]}`;
  return <div>{n}{s}</div>;
}
