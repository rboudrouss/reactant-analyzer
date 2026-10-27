// oracle: react=converges
// The memo recomputes when `s` moves but is `3` every time, so the effect
// runs once: a dep holding one value moves with nothing.
import { useEffect, useMemo, useState } from "react";

export function App() {
  const [s, setS] = useState(0);
  const m = useMemo(() => 3, [s]);
  useEffect(() => {
    setS((p) => p + 1);
  }, [m]);
  return <div>{s}</div>;
}
