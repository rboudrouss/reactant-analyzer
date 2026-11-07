// oracle: react=converges
// #199: the fresh dep recomputes the memo at every render, but the body hands
// back `0` every time, so storing it settles. Recomputing is not changing.
import { useEffect, useMemo, useState } from "react";

export function App() {
  const [s, setS] = useState<number>(0);
  const m = useMemo(() => 0, [{}]);
  useEffect(() => {
    setS(m);
  }, [s]);
  return <div>{s}</div>;
}
