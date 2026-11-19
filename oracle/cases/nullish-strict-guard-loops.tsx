// oracle: react=loop
// #303, the neighbour of `nullish-loose-guard-settles`: `undefined !== null`,
// so the strict guard holds after every write of `{ x: undefined }`, a new
// object each time, and the effect re-runs without end.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState<{ x?: number | null }>({ x: 1 });
  useEffect(() => {
    if (s.x !== null) setS({ x: undefined });
  }, [s]);
  return <div>{s.x}</div>;
}
