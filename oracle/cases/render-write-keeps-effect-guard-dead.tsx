// oracle: react=converges
// #162 (1), the precision side: the render write is a site, but it stores `2`,
// which keeps the effect's `s === null` dead; the effect fires once, and the
// render guard dies under the one write left.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState<number | null>(null);
  if (s === 1) setS(2);
  useEffect(() => {
    if (s === null) setS(1);
  });
  return <div>{s}</div>;
}
