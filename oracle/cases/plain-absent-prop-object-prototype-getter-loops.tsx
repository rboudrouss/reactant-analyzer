// oracle: react=loop
// #336, the looping twin: a getter the program installs on
// `Object.prototype` under the name of the prop the parent does not pass
// runs at the child's read of it, a new object at every render.
import { useEffect, useState } from "react";

Object.defineProperty(Object.prototype, "hide", {
  get() {
    return {};
  },
  configurable: true,
});

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
