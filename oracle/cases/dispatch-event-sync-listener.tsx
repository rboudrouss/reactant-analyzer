// oracle: react=loop
// `dispatchEvent` runs the target's listeners before it returns (#219): the
// listener's write happens in the effect, every effect run re-renders, and the
// effect has no deps. The listener was classified `Handler` (event-only) and
// carried no churn edge, so the analyzer printed `verified infinite-loop`.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({});
  useEffect(() => {
    const h = () => setS({});
    window.addEventListener("ping", h);
    const ev = document.createEvent("Event");
    ev.initEvent("ping");
    window.dispatchEvent(ev);
    return () => window.removeEventListener("ping", h);
  });
  return <div>{Object.keys(s).length}</div>;
}
