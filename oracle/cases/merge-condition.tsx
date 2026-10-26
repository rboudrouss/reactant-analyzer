// oracle: react=loop
// #197: `v` is `obj` or `null` as `a` says, so every toggle of `a` moves `v`
// and re-runs the effect that toggles `a`. A merge takes its condition's
// change set: `Versioned({obj})` alone had no edge into `a`.
import { useEffect, useState } from "react";

export function App() {
  const [a, setA] = useState(false);
  const [obj, setObj] = useState({ k: 1 });
  const v = a ? obj : null;
  useEffect(() => {
    setA((x) => !x);
  }, [v]);
  return <div onClick={() => setObj({ k: 2 })}>{String(a)}</div>;
}
