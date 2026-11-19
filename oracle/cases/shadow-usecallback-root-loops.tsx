// oracle: react=loop
// #310: a `useCallback` body keeps no env of its own: its `x` is not the
// render's.
import { useCallback, useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  const x = 5;
  const bump = useCallback(() => {
    const x = s0 + 1;
    setS0(x);
  }, [s0]);
  useEffect(() => {
    bump();
  }, [s0, x]);
  return <div>{s0}</div>;
}
