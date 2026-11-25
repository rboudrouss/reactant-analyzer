// oracle: react=loop
// #407, N4's neighbour: the effect dispatches the event the listener is
// registered under, both names read through module constants: the listener
// stores a new object, every run re-renders and runs the effect again.
import { useEffect, useState } from "react";

const PING = "ping";
const ALSO_PING = "ping";

export function App() {
  const [n, setN] = useState<unknown>(0);
  useEffect(() => {
    const h = () => setN({});
    window.addEventListener(PING, h);
    return () => window.removeEventListener(PING, h);
  }, []);
  useEffect(() => {
    const NativeEvent = window.Event;
    window.dispatchEvent(new NativeEvent(ALSO_PING));
  });
  return <div>{String(!!n)}</div>;
}
