// oracle: react=converges
// #336. The parent does not pass `hide`: the child's read of it is a read
// of `Object.prototype`, where a `Proxy` elsewhere in the program cannot be.
// It is `undefined` at every render, so the effect runs once.
import { useEffect, useState } from "react";

export const trap: any = new Proxy({}, { get: () => ({}) });

function C({ hide }: { hide?: object }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [hide]);
  return <div>{n}</div>;
}

export function App() {
  return <C />;
}
