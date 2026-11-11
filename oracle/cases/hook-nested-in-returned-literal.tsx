// oracle: react=loop
// #233: a hook call nested in the hook's returned literal. Only a statement
// that is a hook call was extracted, so this `useCallback` was a call of an
// unknown function and `inc()` in the effect ran nothing.
import { useCallback, useEffect, useState } from "react";

function useCounter() {
  const [v, setV] = useState(0);
  return [v, useCallback(() => setV(v + 1), [v])] as const;
}

export function App() {
  const [v, inc] = useCounter();
  useEffect(() => {
    inc();
  });
  return <div>{v}</div>;
}
