// oracle: react=converges
// #407, N4: the listener is registered under a module constant's name,
// `ping`, and the effect dispatches `pong`: nothing hears it. An event name
// written through a module `const` of a string is that string.
import { useEffect, useState } from "react";

const PING = "ping";
const PONG = "pong";

export function App() {
  const [n, setN] = useState<unknown>(0);
  useEffect(() => {
    const h = () => setN({});
    window.addEventListener(PING, h);
    return () => window.removeEventListener(PING, h);
  }, []);
  useEffect(() => {
    const NativeEvent = window.Event;
    window.dispatchEvent(new NativeEvent(PONG));
  });
  return <div>{String(!!n)}</div>;
}
