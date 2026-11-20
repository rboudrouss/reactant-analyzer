// oracle: react=loop
// #366, the props side (#336): `Reflect` bound to a name installs a getter on
// `Object.prototype` under the name of the prop the parent does not pass; the
// child's read of it runs it, a new object at every render.
import { useEffect, useState } from "react";

const R = Reflect;
R.defineProperty(Object.prototype, "hide", {
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
