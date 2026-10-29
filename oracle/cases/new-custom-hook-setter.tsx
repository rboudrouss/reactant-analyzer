// oracle: react=loop
// Found by `oracle fuzz --seed 2` (programs 717, 1265, 1472, shrunk). A setter
// returned by a custom hook and called by the component was not seen: the
// component's fixpoint ran 0 iterations and `infinite-loop` was reported as
// verified (#185). The splice now forwards each member of the returned tuple
// to the destructuring, so `setV` is an alias of the hook's setter.
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
