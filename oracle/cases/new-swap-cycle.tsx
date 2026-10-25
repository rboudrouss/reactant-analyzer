// oracle: react=loop
// Found by `oracle fuzz --seed 2` (program 1156, shrunk). Two slots swap their
// values on every run: (1, 0) → (0, 1) → (1, 0)… A finite cycle, the family of
// canon-toggle.tsx; no value grows and no reference is fresh, but neither write
// is proven to settle (#187).
import { useEffect, useState } from "react";

export function App() {
  const [a, setA] = useState(1);
  const [b, setB] = useState(0);
  useEffect(() => {
    setA(b);
    setB(a);
  });
  return <div>{a + b}</div>;
}
