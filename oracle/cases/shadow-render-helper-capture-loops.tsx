// oracle: react=loop
// #310: a render helper captures the render's `x`; the effect that calls
// it binds another `x`, which the helper never sees.
import { useEffect, useState } from "react";

export function App() {
  const [s0, setS0] = useState(1);
  const x = s0 + 1;
  const bump = () => setS0(x);
  useEffect(() => {
    const x = 5;
    bump();
  }, [s0]);
  return <div>{s0}</div>;
}
