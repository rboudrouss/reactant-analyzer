// oracle: react=loop
// #250: the effect fires its own listener (#219), which writes `v + 1`. The
// listener is passed by name, so no handler entry runs it: the fixpoint kept
// `v` at `{0}` and read the write as storing one value, which settles.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  useEffect(() => {
    const h = () => {
      setV(v + 1);
    };
    window.addEventListener("ping", h);
    const ev = document.createEvent("Event");
    ev.initEvent("ping");
    window.dispatchEvent(ev);
    return () => window.removeEventListener("ping", h);
  });
  return <div>{v}</div>;
}
