// oracle: react=converges
// #258, the precision side: nothing calls `arm`, so `ready` stays `false`
// and the guarded write never runs.
import { useEffect, useState } from "react";

export function App() {
  const [n, setN] = useState(0);
  useEffect(() => {
    let ready = false;
    const arm = () => {
      ready = true;
    };
    if (ready) setN(n + 1);
    return () => void arm;
  });
  return <div>{n}</div>;
}
