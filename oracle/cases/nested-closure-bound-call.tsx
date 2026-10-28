// oracle: react=loop
// #246: the value `get()` returns bound to a name of the `forEach` callback,
// then called: a name the callback binds, which no converged env held.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const get = () => setV;
  useEffect(() => {
    [1].forEach(() => {
      const s = get();
      s(v + 1);
    });
  });
  return <div>{v}</div>;
}
