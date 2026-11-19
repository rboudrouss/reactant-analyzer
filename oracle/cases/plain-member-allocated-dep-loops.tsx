// oracle: react=loop
// #321, the plain-member twin: `o.p` is the literal's member, a new object
// at every render, as before.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  const o = { p: {} };
  useEffect(() => {
    setN((c) => c + 1);
  }, [o.p]);
  return <div>{n}</div>;
}
