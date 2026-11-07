// oracle: react=loop
// An element's `click()` dispatches `click` before it returns, as
// `dispatchEvent` does (#219): the listener the effect registers on the
// document runs inside the effect, every effect run re-renders, and the
// effect has no deps.
import { useEffect, useState } from "react";

export function App() {
  const [s, setS] = useState({});
  useEffect(() => {
    const h = () => setS({});
    document.addEventListener("click", h);
    document.body.click();
    return () => document.removeEventListener("click", h);
  });
  return <div>{Object.keys(s).length}</div>;
}
