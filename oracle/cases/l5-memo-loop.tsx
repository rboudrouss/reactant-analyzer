// oracle: react=loop
// L5: the loop runs through a memo; the slot reaches ⊤ by join, not by widening.
import { useEffect, useMemo, useState } from "react";

export function App() {
  const [x, setX] = useState(0);
  const d = useMemo(() => x + 1, [x]);
  useEffect(() => {
    setX(d);
  }, [d]);
  return <p>{x}</p>;
}
