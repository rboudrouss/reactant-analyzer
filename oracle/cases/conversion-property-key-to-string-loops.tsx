// oracle: react=loop
// #360: a computed member access converts its key to a property key, which
// calls the key's `toString`, at a read and at a write alike, with the
// setter, at every render.
import { useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const h = () => setN(n + 1);
  const k = {
    toString: () => {
      h();
      return "a";
    },
  };
  const t: Record<string, number> = { a: 1 };
  t[k as unknown as string] = 2;
  const s = t[k as unknown as string];
  return <div>{n}{s}</div>;
}
