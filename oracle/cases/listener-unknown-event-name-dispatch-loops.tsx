// oracle: react=loop
// #422: a listener registered under a name the analysis does not know (an
// array's entry) hears any event, so the program's `ping` dispatch may reach
// it. It does: the effect registers it under `ping` and dispatches `ping` at
// every commit, and the listener writes a new object.
import { useEffect, useState } from "react";

const NAMES = ["ping"];

export function App() {
  const [n, setN] = useState<unknown>(0);
  useEffect(() => {
    const h = () => setN({});
    window.addEventListener(NAMES[0], h);
    return () => window.removeEventListener(NAMES[0], h);
  });
  useEffect(() => {
    const NativeEvent = window.Event;
    window.dispatchEvent(new NativeEvent("ping"));
  });
  return <div>{String(!!n)}</div>;
}
