// oracle: react=loop
// #360: `o + ""` converts `o` to a primitive: with no `valueOf` of its own,
// `Object.prototype.valueOf` returns the object, and its `toString` runs,
// with the setter, at every render.
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
  const s = o + "";
  return <div>{n}{s}</div>;
}
