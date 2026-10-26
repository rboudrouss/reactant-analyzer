// oracle: react=converges
// #185, the precision side: the forwarded setter is the hook's setter and
// nothing more. Written only by a click handler, it neither loops nor runs
// during render.
import { useState } from "react";

function useSlot() {
  const [v, setV] = useState(0);
  return [v, setV] as const;
}

export function App() {
  const [v, setV] = useSlot();
  return <button onClick={() => setV(v + 1)}>{v}</button>;
}
