// oracle: react=loop
// #246: a call-returned setter called in a `.then` continuation of a no-deps
// effect: a deferred write every effect run, through a temp bound in the
// continuation.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  const get = () => setV;
  useEffect(() => {
    Promise.resolve().then(() => get()(v + 1));
  });
  return <div>{v}</div>;
}
