// oracle: react=loop
// #231, the render-phase form of `guard-on-one-arm-alias.tsx`. `d` is the
// slot on one arm only, and `c` never takes it: `d` stays `null`, the guard
// never dies, and every render stores a new object. `setter-in-render` read
// the guard as settled by the write through the may-alias table.
import { useState } from "react";

export function App() {
  const [s, setS] = useState<{ n: number } | null>(null);
  const [c] = useState(false);
  const d = c ? s : null;
  if (d === null) setS({ n: 1 });
  return <div>{s ? s.n : 0}</div>;
}
