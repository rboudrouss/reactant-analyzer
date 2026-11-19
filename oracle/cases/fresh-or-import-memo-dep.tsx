// oracle: react=loop
// #304: a memo whose dep is `absent ?? {}` recomputes at every render, and
// its body allocates, so the memo is a new object at every render.
import { useEffect, useMemo, useState } from "react";
import { absent } from "ext";

export function App() {
  const [s2, setS2] = useState(0);
  const m = useMemo(() => ({ a: 1 }), [absent ?? {}]);
  useEffect(() => {
    setS2(4 - s2);
  }, [m]);
  return <div>{s2}</div>;
}
