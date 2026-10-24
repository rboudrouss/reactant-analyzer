// oracle: react=loop
// oracle: known=fn
// Found by `oracle fuzz --seed 2` (programs 717, 1265, 1472, shrunk). A setter
// returned by a custom hook and called by the component is not seen: the
// component's fixpoint runs 0 iterations and `infinite-loop` is reported as
// verified. The same effect written inside the hook is found; returning
// `{ v, setV }` instead of a tuple is missed the same way.
import { useEffect, useState } from "react";

function useSlot() {
  const [v, setV] = useState(0);
  return [v, setV];
}

export function App() {
  const [v, setV] = useSlot();
  useEffect(() => {
    setV(v + 1);
  });
  return <div>{v}</div>;
}
