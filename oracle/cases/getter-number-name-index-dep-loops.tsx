// oracle: react=loop
// #336, the looping twin: the accessor is named `0`, the key a number's, so
// `o[i % 2]` runs the getter, a new object at every render, and the effect
// re-runs forever.
import { useEffect, useState } from "react";

const o = {
  get 0() {
    return {};
  },
};

export function App() {
  const [i] = useState(0);
  const [n, setN] = useState(0);
  useEffect(() => {
    setN((c) => c + 1);
  }, [o[i % 2]]);
  return <div>{n}</div>;
}
