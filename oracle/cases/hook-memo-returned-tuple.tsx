// oracle: react=loop
// #233: a hook returning a memoized tuple. The memo's value was its body's
// value without the array it returns, so `setV` read off it was ⊤ and the
// write in the effect landed nowhere.
import { useEffect, useMemo, useState } from "react";

function useSlot() {
  const [v, setV] = useState(0);
  return useMemo(() => [v, setV] as const, [v]);
}

export function App() {
  const [v, setV] = useSlot();
  useEffect(() => {
    setV(v + 1);
  });
  return <div>{v}</div>;
}
