// oracle: react=loop
// #162 (1). The render resets the slot the effect's guard waits on: the effect
// (no deps array) stores an object, the render puts `null` back, and the next
// commit runs the effect on `null` again. Without the render write the guard
// dies once written; the render write is a site of the slot and revives it.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState<{ n: number } | null>(null);
  if (s) setS(null);
  useEffect(() => {
    if (!s) setS({ n: 1 });
  });
  return <div>{s ? s.n : 0}</div>;
}
