// oracle: react=converges
// #250: no body of the program fires an event, so the listener runs only on
// a user event: the write it makes is no loop.
import { useEffect, useState } from "react";

export function App() {
  const [v, setV] = useState(0);
  useEffect(() => {
    const h = () => {
      setV(v + 1);
    };
    window.addEventListener("ping", h);
    return () => window.removeEventListener("ping", h);
  });
  return <div>{v}</div>;
}
