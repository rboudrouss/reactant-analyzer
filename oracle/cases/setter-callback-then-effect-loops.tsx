// oracle: react=loop
// #291, the `then` variant of setter-callback-foreach-effect-loops.tsx: the
// promise calls the setter with what it resolves to, a tick later.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  useEffect(() => {
    Promise.resolve(v + 1).then(setV);
  }, [v]);
  return <div>{v}</div>;
}
