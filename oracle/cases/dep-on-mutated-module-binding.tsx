// oracle: react=loop
// #224. `cache.size` moves when the effect mutates the module-level map, with
// no slot or ref behind it: each run grows the map and stores a new array,
// which re-renders onto a moved dep. The dep was read as held, so the effect
// as never re-running.
import { useEffect, useState } from "react";

const cache = new Map<number, number>();

export function App() {
  const [s, setS] = useState<number[]>([]);
  useEffect(() => {
    cache.set(cache.size, 1);
    setS([]);
  }, [cache.size]);
  return <div>{s.length}</div>;
}
