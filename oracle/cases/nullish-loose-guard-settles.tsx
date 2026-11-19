// oracle: react=converges
// #303. The effect writes `{ x: undefined }` while `s.x != null`; after the
// write `s.x` is `undefined`, which the loose guard rejects, so the effect
// re-runs once and stops. A guard read as strict (`!== null`) would hold again.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState<{ x?: number }>({ x: 1 });
  useEffect(() => {
    if (s.x != null) setS({ x: undefined });
  }, [s]);
  return <div>{s.x}</div>;
}
