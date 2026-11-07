// oracle: react=loop
// #162 (1), conditional: the render reset sits behind a prop. Whether it runs
// depends on the parent, so the claim is a Warning, never an Error.
import { useEffect, useState } from "react";

export function App({ reset = true }: { reset?: boolean }) {
  const [s, setS] = useState<{ n: number } | null>(null);
  if (reset && s) setS(null);
  useEffect(() => {
    if (!s) setS({ n: 1 });
  });
  return <div>{s ? s.n : 0}</div>;
}
