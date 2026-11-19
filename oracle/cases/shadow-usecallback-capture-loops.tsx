// oracle: react=loop
// #310: a `useCallback` body captures the render's `x`; the effect that
// calls it binds another `x`.
import { useCallback, useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  const x = s0 + 1;
  const bump = useCallback(() => setS0(x), [x]);
  useEffect(() => {
    const x = 5;
    bump();
  }, [s0]);
  return <div>{s0}</div>;
}
