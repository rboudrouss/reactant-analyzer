// oracle: react=loop
// #246: a call-returned setter called in a closure nested in a listener the
// effect dispatches to itself (#219): the listener runs in the effect, and its
// inner `forEach` callback with it, storing a new object each time.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({});
  const get = () => setS;
  useEffect(() => {
    const h = () => {
      [1].forEach(() => get()({}));
    };
    window.addEventListener("ping", h);
    const ev = document.createEvent("Event");
    ev.initEvent("ping");
    window.dispatchEvent(ev);
    return () => window.removeEventListener("ping", h);
  });
  return <div>{Object.keys(s).length}</div>;
}
