// oracle: react=loop
// #216: a callback inside an inline object prop. The literal `{ tick: cb }`
// got no heap object, so the child's `api.tick()` resolved to nothing: the
// parent's closure never ran, and its write into `n` reached no store.
import { useEffect, useState } from "react";

function Child({ api }: { api: { tick: () => void } }) {
  useEffect(() => {
    api.tick();
  });
  return null;
}

export function App() {
  const [n, setN] = useState(0);
  const cb = () => setN(n + 1);
  return <Child api={{ tick: cb }} />;
}
