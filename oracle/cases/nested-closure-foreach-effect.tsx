// oracle: react=loop
// #246: a call-returned setter called in a `forEach` callback of a no-deps
// effect. The temp `get()` is bound to lives in the callback, a body no
// converged env held, so the write had no row.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const get = () => setV;
  useEffect(() => {
    [1].forEach(() => get()(v + 1));
  });
  return <div>{v}</div>;
}
