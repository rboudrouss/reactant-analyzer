// oracle: react=loop
// `k` captures `x`, which the render reassigns after `k` is created; the
// effect calls `k` through `run`, which does not capture `x`. JavaScript
// closures capture variables, so `k` reads 5 at the call (#215), and the
// write arms the growing effect. A closure of the component runs in the
// component's scope, live at the call, whatever body calls it (#326).
import { useEffect, useState } from "react";

export function App() {
  const [d, setD] = useState(0);
  let x = 0;
  const k = () => setD(x);
  const run = () => k();
  x = 5;
  useEffect(() => {
    run();
  }, []);
  useEffect(() => {
    if (d > 0) setD(d + 1);
  }, [d]);
  return <div>{d}</div>;
}
