// oracle: react=converges
// #246, the precision side: the closure that calls `get()(…)` is built in the
// effect and in a click handler, and nothing ever runs it. A closure's names
// resolve through the env it is reached from; that does not make it run.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const get = () => setV;
  useEffect(() => {
    const later = () => get()(v + 1);
    return () => void later;
  });
  return <button onClick={() => [1].forEach(() => get()(v + 1))}>{v}</button>;
}
