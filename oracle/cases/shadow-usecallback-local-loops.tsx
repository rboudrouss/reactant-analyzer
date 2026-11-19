// oracle: react=loop
// #310: a `useCallback` body binds its own `x`; the effect that calls it
// binds another.
import { useCallback, useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  const bump = useCallback(() => {
    const x = s0 + 1;
    setS0(x);
  }, [s0]);
  useEffect(() => {
    const x = 5;
    bump();
  }, [s0]);
  return <div>{s0}</div>;
}
