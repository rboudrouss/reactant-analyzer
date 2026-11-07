// oracle: react=converges
// #250: the effect fires its own listener, which stores one value: once it
// sits in the slot, the next run stores it again and React bails out.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  useEffect(() => {
    const h = () => {
      setV(1);
    };
    window.addEventListener("ping", h);
    const ev = document.createEvent("Event");
    ev.initEvent("ping");
    window.dispatchEvent(ev);
    return () => window.removeEventListener("ping", h);
  });
  return <div>{v}</div>;
}
